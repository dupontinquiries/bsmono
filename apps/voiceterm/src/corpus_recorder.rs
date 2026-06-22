// src/corpus_recorder.rs
//
// v0.1.192 (ship 2.3 of voice corpus arc) — discrete audio recording
// during reading-mode sessions. Distinct from the continuous voice
// pipeline in voice.rs; tapes a bounded recording (start → stop)
// and saves it to disk along with the reference paragraph text.
//
// Design:
//
//   - Global state (a few Mutex/atomics) — only one corpus
//     recording active at a time per process.
//   - Audio sample callbacks in voice.rs push raw f32 samples
//     here when `is_active()` is true. Zero overhead when inactive.
//   - `start_recording` clears the buffer and stamps metadata.
//   - `stop_recording` spawns a background thread to write the
//     .wav and .txt files, so the UI/audio threads aren't blocked.
//   - 60-second cap on a single recording. Long enough for a
//     50-word paragraph at 250 wpm (~12s) with lots of headroom.
//     Stops gracefully if exceeded.
//
// Storage:
//
//   ~/.config/web2qml/corpus/<speaker_id>/<unix_ms>.wav
//   ~/.config/web2qml/corpus/<speaker_id>/<unix_ms>.txt
//
// The .txt holds the paragraph text — the ground-truth reference
// for whatever this audio was supposed to be saying. Used later
// for whisper-finetune / per-speaker calibration / corpus analysis.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// True while a corpus recording is active. Audio callbacks check
/// this with relaxed atomic load — minimum cost on the hot path.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// v0.1.287: pending writer-thread JoinHandle. Set by
/// stop_recording when a write is spawned; consumed by flush()
/// on shutdown so the process doesn't terminate mid-write.
///
/// We only track ONE handle at a time. start_recording's "stop
/// existing first" semantics mean writer threads can't overlap
/// in normal use; if a hyper-fast start/stop loop somehow
/// outpaced a previous write, the new spawn would clobber the
/// old handle and the old thread runs to completion detached
/// (best-effort, no guarantees).
fn pending_writer() -> &'static Mutex<Option<std::thread::JoinHandle<()>>> {
    static P: OnceLock<Mutex<Option<std::thread::JoinHandle<()>>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

/// Sample rate at start of the recording. Stored as the audio
/// pipeline may run at different rates depending on device caps.
/// We trust the first sample-batch's rate and stick with it for
/// the duration of one recording.
static SAMPLE_RATE: AtomicU32 = AtomicU32::new(0);

/// Unix ms timestamp when start_recording was called. Used as the
/// filename stem.
static STARTED_AT_MS: AtomicU64 = AtomicU64::new(0);

/// Max samples we'll buffer. At 48kHz that's 60 seconds; at 16kHz
/// it's ~180 seconds. Either way far more than a 50-word paragraph
/// needs. Hard cap so a forgotten recording doesn't grow unbounded.
const MAX_SAMPLES: usize = 48_000 * 60;

struct State {
    samples: Vec<f32>,
    paragraph_text: String,
    speaker_id: String,
    /// v0.1.325 (cards arc 2.5.1): URL of the page the user was
    /// reading when the recording was captured. Empty when the
    /// recording wasn't triggered from a page context (rare —
    /// reading mode is the normal entry point). Persisted in
    /// the .txt sidecar header so the connections graph can
    /// emit `VoiceFromPage` edges later.
    source_url: String,
    /// v0.1.325 (cards arc 2.5.1): page title at recording
    /// time. Same trade-off as source_url — best-effort.
    source_title: String,
    /// v0.1.510: the active voice-grammar label at record time
    /// (e.g. "editor:helix", "repl:gdb", "shell", "web"), from
    /// grammar_context::grammar_label. Stamped into the .txt sidecar
    /// so the corpus self-labels ("show my helix voice commands").
    /// Empty when not supplied (the plain start_recording path).
    grammar: String,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State {
        samples: Vec::new(),
        paragraph_text: String::new(),
        speaker_id: String::new(),
        source_url: String::new(),
        source_title: String::new(),
        grammar: String::new(),
    }))
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Check if a corpus recording is currently active. Called by the
/// audio callbacks in voice.rs every chunk. Must be cheap.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Start a new recording session. If one is already active, this
/// stops it (silently, no file saved) before starting a new one.
/// The paragraph_text is stored alongside the audio for ground
/// truth in downstream training/calibration.
///
/// v0.1.325 (cards arc 2.5.1): this is now a thin wrapper around
/// `start_recording_with_source` for back-compat (no URL/title).
/// New callers should use the full form when they know the page
/// context — the connections graph emits VoiceFromPage edges
/// when both are recorded.
pub fn start_recording(paragraph_text: &str, speaker_id: &str) {
    start_recording_with_source(paragraph_text, speaker_id, "", "")
}

/// v0.1.325: start a recording with page-context metadata. The
/// URL + title get embedded in the .txt sidecar header so the
/// connections graph can emit VoiceFromPage edges.
pub fn start_recording_with_source(
    paragraph_text: &str,
    speaker_id: &str,
    source_url: &str,
    source_title: &str,
) {
    // v0.1.510: delegate to the grammar-aware form with no grammar,
    // so existing callers are unchanged.
    start_recording_with_context(paragraph_text, speaker_id, source_url, source_title, "")
}

/// v0.1.510: start a recording, also stamping the active voice-grammar
/// label (grammar_context::grammar_label) into the sidecar header so
/// the corpus self-labels. `grammar` may be empty (no label emitted).
pub fn start_recording_with_context(
    paragraph_text: &str,
    speaker_id: &str,
    source_url: &str,
    source_title: &str,
    grammar: &str,
) {
    // If somehow we're starting while already active, just discard
    // the prior session. Most likely from QML side resetting state
    // without going through stop.
    if ACTIVE.load(Ordering::Relaxed) {
        crate::clog!("corpus", "start while already active — discarding prior session");
    }
    if let Ok(mut s) = state().lock() {
        s.samples.clear();
        s.samples.reserve(48_000 * 30); // 30s at 48kHz — likely overhead
        s.paragraph_text = paragraph_text.to_string();
        s.speaker_id = if speaker_id.is_empty() {
            "unknown".to_string()
        } else {
            speaker_id.to_string()
        };
        s.source_url = source_url.to_string();
        s.source_title = source_title.to_string();
        s.grammar = grammar.to_string();
    }
    STARTED_AT_MS.store(unix_ms(), Ordering::Relaxed);
    SAMPLE_RATE.store(0, Ordering::Relaxed); // will be set on first sample batch
    ACTIVE.store(true, Ordering::Relaxed);
    crate::clog!("corpus", "start recording (speaker={}, paragraph={} chars, url={})",
        speaker_id, paragraph_text.len(),
        if source_url.is_empty() { "(none)" } else { source_url });
}

/// Push raw mono f32 samples into the active recording. Called from
/// the cpal audio callback in voice.rs. No-op when not active.
///
/// First call records the sample rate; subsequent calls assume it
/// stays the same (cpal doesn't change rate mid-stream).
pub fn push_samples(samples: &[f32], sample_rate: u32) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    // Record the rate from the first non-empty batch.
    if SAMPLE_RATE.load(Ordering::Relaxed) == 0 && sample_rate > 0 {
        SAMPLE_RATE.store(sample_rate, Ordering::Relaxed);
    }
    if let Ok(mut s) = state().lock() {
        if s.samples.len() + samples.len() > MAX_SAMPLES {
            // Hit the cap. Stop accumulating; the stop_recording call
            // will write what we have. Don't mark inactive here — the
            // caller (QML side) will explicitly stop.
            return;
        }
        s.samples.extend_from_slice(samples);
    }
}

/// Stop the active recording. Spawns a background thread to write
/// the .wav and .txt files so callers don't block on disk I/O.
/// No-op if not active.
pub fn stop_recording() {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    ACTIVE.store(false, Ordering::Relaxed);

    // Snapshot the state — we'll move it into the writer thread so
    // a new recording can start immediately without waiting on the
    // disk write.
    let (samples, paragraph_text, speaker_id, source_url, source_title, grammar, sample_rate, started_at) = {
        let mut s = match state().lock() {
            Ok(s) => s,
            Err(_) => {
                crate::clog!("corpus", "state lock poisoned, dropping recording");
                return;
            }
        };
        let samples = std::mem::take(&mut s.samples);
        let paragraph_text = std::mem::take(&mut s.paragraph_text);
        let speaker_id = std::mem::take(&mut s.speaker_id);
        let source_url = std::mem::take(&mut s.source_url);
        let source_title = std::mem::take(&mut s.source_title);
        let grammar = std::mem::take(&mut s.grammar);
        (
            samples,
            paragraph_text,
            speaker_id,
            source_url,
            source_title,
            grammar,
            SAMPLE_RATE.load(Ordering::Relaxed),
            STARTED_AT_MS.load(Ordering::Relaxed),
        )
    };

    if samples.is_empty() {
        crate::clog!("corpus", "stop with empty buffer; nothing to write");
        return;
    }
    if sample_rate == 0 {
        crate::clog!("corpus", "stop with no sample rate recorded; dropping {} samples",
            samples.len());
        return;
    }

    // Background thread writes the files. The audio pipeline keeps
    // running unimpeded.
    //
    // v0.1.287: store the JoinHandle in pending_writer() so a
    // process shutdown (Backend::drop, deliberate exit) can flush()
    // it before the process terminates. Without this, an exit
    // racing the writer thread could leave a half-written wav.
    let handle = std::thread::Builder::new()
        .name("corpus-writer".to_string())
        .spawn(move || {
            // v0.1.515: this writer is disk-bound (WAV write + opus
            // encode) and the user never waits on it.
            // v0.1.517: gate the E-core bias on screen-off
            // (is_app_backgrounded) for CONSISTENCY with the other
            // background corpus/index ops — when the user is present we
            // let it run full-speed (prompt durability if the app is
            // killed); when backgrounded we take the efficiency path.
            // Guard lifts on drop. Best-effort; no-op if unsupported.
            let _eco = crate::cpu_efficiency::EcoGuard::new(
                crate::voice::is_app_backgrounded());
            if let Err(e) = write_recording(samples, paragraph_text, speaker_id,
                source_url, source_title, grammar, sample_rate, started_at) {
                crate::clog!("corpus", "write failed: {}", e);
            }
        })
        .ok();
    if let Some(h) = handle {
        if let Ok(mut g) = pending_writer().lock() {
            // If a prior writer is still in flight (rare —
            // requires a stop→start→stop cycle inside one
            // disk-write window), let it continue detached.
            // We can't join two; pick the most recent so flush()
            // covers it.
            *g = Some(h);
        }
    }
}

/// v0.1.287: block until any pending writer-thread finishes.
/// Call from Backend::drop / explicit shutdown so the process
/// doesn't terminate while a corpus recording is still being
/// written to disk.
///
/// Idempotent — calling with no pending writer is a no-op.
/// Timeout: none. Writes are bounded by disk speed × audio
/// duration; for a 60-second recording at 48kHz that's
/// ~10-20MB and finishes in well under 1s. Acceptable
/// shutdown cost.
/// v0.1.359: expose the most recently observed sample rate from
/// the audio pipeline. Used by ad-hoc audio writers (e.g. the
/// onboarding spoken-name capture) that want to match the format
/// of the live recorder without setting up their own capture.
/// Returns None when the recorder has never been pushed samples
/// in this session, so callers can fall back to a sensible
/// default (typically 16000 for whisper-friendly output).
pub fn current_sample_rate() -> Option<u32> {
    let sr = SAMPLE_RATE.load(Ordering::Relaxed);
    if sr == 0 { None } else { Some(sr) }
}

/// v0.1.359: write a WAV file directly without going through the
/// usual start/push/stop recorder lifecycle. Used by the
/// onboarding spoken-name path, which takes audio that's already
/// been captured into the backend's `last_audio_buffer` and just
/// needs it on disk under a non-timestamped predictable filename.
///
/// Format: 16-bit signed PCM, mono. Same encoder as the internal
/// `write_wav` (which this is a thin wrapper around).
pub fn write_wav_public(
    path: &std::path::PathBuf,
    samples: &[f32],
    sample_rate: u32,
) -> std::io::Result<()> {
    write_wav(path, samples, sample_rate)
}

pub fn flush_writer() {
    let handle_opt = match pending_writer().lock() {
        Ok(mut g) => g.take(),
        Err(_) => return,
    };
    if let Some(h) = handle_opt {
        if let Err(e) = h.join() {
            crate::clog!("corpus", "writer thread panicked during shutdown: {:?}", e);
        }
    }
}

/// v0.1.511: parse a .txt sidecar into (grammar_label, body_text),
/// stripping the optional KEY: header (URL/TITLE/GRAMMAR ... `---`).
/// Mirrors connections::parse_voice_txt's key-order-tolerant rules so
/// list_recordings doesn't bleed header lines into paragraph_text (a
/// regression once v0.1.510 added headers to recordings that have a
/// grammar but no page source). Returns ("", whole_content) for
/// header-less files.
fn parse_sidecar(content: &str) -> (String, String) {
    fn header_kv(line: &str) -> Option<(&'static str, &str)> {
        for key in ["URL: ", "TITLE: ", "GRAMMAR: "] {
            if let Some(v) = line.strip_prefix(key) {
                return Some((key, v.trim()));
            }
        }
        None
    }
    let mut lines = content.lines();
    let first = match lines.next() {
        Some(l) => l,
        None => return (String::new(), String::new()),
    };
    if header_kv(first).is_none() {
        // No header — whole content is body.
        return (String::new(), content.to_string());
    }
    let mut grammar = String::new();
    if let Some(("GRAMMAR: ", v)) = header_kv(first) { grammar = v.to_string(); }
    let mut body = String::new();
    let mut body_started = false;
    for line in lines {
        if !body_started {
            if line.trim() == "---" { body_started = true; continue; }
            if let Some((k, v)) = header_kv(line) {
                if k == "GRAMMAR: " { grammar = v.to_string(); }
                continue;
            }
            body_started = true;
            body.push_str(line); body.push('\n');
            continue;
        }
        body.push_str(line); body.push('\n');
    }
    while body.ends_with('\n') { body.pop(); }
    (grammar, body)
}

fn write_recording(
    samples: Vec<f32>,
    paragraph_text: String,
    speaker_id: String,
    source_url: String,
    source_title: String,
    grammar: String,
    sample_rate: u32,
    started_at_ms: u64,
) -> std::io::Result<()> {
    let home = std::env::var("HOME").map_err(|_|
        std::io::Error::new(std::io::ErrorKind::Other, "HOME unset"))?;
    let dir = PathBuf::from(home)
        .join(".config/web2qml/corpus")
        .join(&speaker_id);
    std::fs::create_dir_all(&dir)?;

    let stem = started_at_ms.to_string();
    let wav_path = dir.join(format!("{}.wav", stem));
    let txt_path = dir.join(format!("{}.txt", stem));

    write_wav(&wav_path, &samples, sample_rate)?;

    // v0.1.325 (cards arc 2.5.1): if we have source page
    // metadata, prepend a small header before the paragraph text.
    // v0.1.510: header now also carries GRAMMAR (the active voice
    // grammar when recorded) so the corpus self-labels. The parser
    // (connections::parse_voice_txt) is key-ORDER-TOLERANT, so we emit
    // only the keys we have, in a stable order, followed by the `---`
    // marker. A header exists if ANY key is present (URL or GRAMMAR);
    // pre-header files (no keys) still parse as all-body.
    //
    //     URL: <url>
    //     TITLE: <title>
    //     GRAMMAR: <label>
    //     ---
    //     <paragraph text>
    //
    // Newlines in fields are stripped defensively — they'd confuse
    // the line-based parser.
    let san = |s: &str| s.replace('\n', " ").replace('\r', "");
    let mut header = String::new();
    if !source_url.is_empty() {
        header.push_str(&format!("URL: {}\n", san(&source_url)));
        header.push_str(&format!("TITLE: {}\n", san(&source_title)));
    }
    if !grammar.is_empty() {
        header.push_str(&format!("GRAMMAR: {}\n", san(&grammar)));
    }
    let body = if header.is_empty() {
        paragraph_text
    } else {
        format!("{}---\n{}", header, paragraph_text)
    };
    std::fs::write(&txt_path, &body)?;

    let secs = samples.len() as f64 / sample_rate as f64;
    crate::clog!("corpus", "wrote {} ({:.1}s, {} samples, {} Hz)",
        wav_path.display(), secs, samples.len(), sample_rate);

    // v0.1.231: opus conversion. WAV at 16-bit mono is ~88KB/sec;
    // opus at 32kbps speech-quality is ~4KB/sec — ~22x smaller for
    // the same audio. The corpus accumulates fast; user flagged
    // this as ASAP.
    //
    // Strategy: shell out to ffmpeg AFTER the WAV is committed.
    // - No new compile-time deps (ffmpeg is system-installed on
    //   most Linux including Asahi Fedora)
    // - Capture path stays WAV-only, so the audio thread can't
    //   ever block on encoder errors
    // - If ffmpeg is missing, files stay as WAV — the lister
    //   handles both extensions, nothing breaks
    // - If ffmpeg is present, the wav is deleted after successful
    //   conversion. We never delete the wav unless opus exists
    //   and passes a sanity check (>= 100 bytes).
    //
    // This is in the same background thread as the WAV write
    // (spawned by stop_recording), so we can call ffmpeg
    // synchronously here without blocking the audio runtime.
    let opus_path = dir.join(format!("{}.opus", stem));
    convert_wav_to_opus(&wav_path, &opus_path);

    Ok(())
}

/// v0.1.231: best-effort WAV → opus conversion via ffmpeg.
///
/// Quietly no-ops (leaving the WAV in place) if:
///   - ffmpeg isn't on PATH
///   - the conversion process returns non-zero exit
///   - the opus file is suspiciously small (<100 bytes — likely
///     a header-only partial write)
///
/// Logs once per attempt so the user can see what's happening.
/// Voice-thread safety: this runs in the writer background
/// thread that was spawned by `stop_recording`, never in the
/// audio capture callback.
fn convert_wav_to_opus(wav_path: &PathBuf, opus_path: &PathBuf) {
    // Bitrate 32kbps is the recommended floor for clear speech.
    // libopus's `--application voip` and `voice-bitrate 24kbps`
    // are also reasonable; we pick 32k for safety. Sample rate
    // is preserved from the input WAV.
    let output = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel", "error",
            "-y",                              // overwrite output if exists
            "-i", &wav_path.to_string_lossy(),
            "-c:a", "libopus",
            "-b:a", "32k",
            "-application", "voip",
            &opus_path.to_string_lossy(),
        ])
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            crate::clog!("corpus", "opus: ffmpeg not available ({}); leaving WAV", e);
            return;
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        crate::clog!("corpus", "opus: ffmpeg failed (status={:?}): {}",
            output.status.code(), stderr.trim());
        // Clean up any partial output file ffmpeg might have left.
        let _ = std::fs::remove_file(opus_path);
        return;
    }
    // Sanity-check the output size before deleting the WAV. A
    // <100 byte file is either an empty container or a broken
    // partial write — keep the WAV in that case.
    let opus_size = match std::fs::metadata(opus_path) {
        Ok(m) => m.len(),
        Err(_) => {
            crate::clog!("corpus", "opus: output file missing after ffmpeg exit-ok; keeping WAV");
            return;
        }
    };
    if opus_size < 100 {
        crate::clog!("corpus", "opus: output suspiciously small ({} bytes); keeping WAV", opus_size);
        let _ = std::fs::remove_file(opus_path);
        return;
    }
    // Conversion successful — delete the WAV to reclaim space.
    let wav_size = std::fs::metadata(wav_path).map(|m| m.len()).unwrap_or(0);
    match std::fs::remove_file(wav_path) {
        Ok(_) => {
            let ratio = if opus_size > 0 { wav_size as f32 / opus_size as f32 } else { 0.0 };
            crate::clog!("corpus", "opus: {} ({} → {} bytes, {:.1}x smaller)",
                opus_path.display(), wav_size, opus_size, ratio);
        }
        Err(e) => {
            crate::clog!("corpus", "opus: encoded ok but couldn't remove source WAV: {}", e);
        }
    }
}

/// Write a minimal PCM WAV file: 44-byte header + 16-bit
/// little-endian samples. Mono. Standard format readable by
/// anything that reads .wav.
fn write_wav(path: &PathBuf, samples: &[f32], sample_rate: u32) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;

    let bits_per_sample: u16 = 16;
    let channels: u16 = 1;
    let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
    let block_align = channels * bits_per_sample / 8;
    let data_size = (samples.len() * 2) as u32; // 2 bytes per i16 sample
    let chunk_size = 36 + data_size;

    // RIFF header
    f.write_all(b"RIFF")?;
    f.write_all(&chunk_size.to_le_bytes())?;
    f.write_all(b"WAVE")?;

    // fmt subchunk
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;       // subchunk size
    f.write_all(&1u16.to_le_bytes())?;        // PCM format
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&block_align.to_le_bytes())?;
    f.write_all(&bits_per_sample.to_le_bytes())?;

    // data subchunk
    f.write_all(b"data")?;
    f.write_all(&data_size.to_le_bytes())?;

    // Quantize and write samples. Clamp to [-1.0, 1.0] before
    // converting; some pipelines deliver values slightly outside.
    let mut buf = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let q = (clamped * 32767.0) as i16;
        buf.extend_from_slice(&q.to_le_bytes());
    }
    f.write_all(&buf)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inactive_by_default() {
        assert!(!is_active());
    }

    #[test]
    fn start_then_active() {
        start_recording("hello world", "spk_test");
        assert!(is_active());
        // Clean up so other tests don't see stale state.
        ACTIVE.store(false, Ordering::Relaxed);
    }

    #[test]
    fn push_when_inactive_noop() {
        ACTIVE.store(false, Ordering::Relaxed);
        push_samples(&[0.1, 0.2, 0.3], 48000);
        // No assertion needed — just verifying it doesn't panic.
    }

    // v0.1.230: corpus listing tests. We don't write actual
    // files in tests (would litter the user's real corpus dir or
    // require a tempdir setup); these test the markdown
    // generation path with synthetic CorpusEntry data and the
    // format_unix_ms relative-time formatter.

    #[test]
    fn format_unix_ms_emits_relative() {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // 30 seconds ago.
        let s = format_unix_ms(now_ms.saturating_sub(30_000));
        assert!(s.ends_with("s ago"), "got: {}", s);
        // 5 minutes ago.
        let s = format_unix_ms(now_ms.saturating_sub(5 * 60_000));
        assert!(s.ends_with("m ago"), "got: {}", s);
        // 3 hours ago.
        let s = format_unix_ms(now_ms.saturating_sub(3 * 3600_000));
        assert!(s.ends_with("h ago"), "got: {}", s);
        // 2 days ago.
        let s = format_unix_ms(now_ms.saturating_sub(2 * 86400_000));
        assert!(s.ends_with("d ago"), "got: {}", s);
    }

    #[test]
    fn format_unix_ms_handles_future_clock_skew() {
        // If timestamp > now (clock weirdness), don't crash —
        // emit a raw ts marker.
        let s = format_unix_ms(u64::MAX);
        assert!(s.starts_with("ts="), "got: {}", s);
    }

    #[test]
    fn list_recordings_markdown_handles_empty() {
        // We can't trivially zero the corpus dir for this test
        // (would mess with the user's real recordings), so we
        // can't strictly test the empty path without mocking.
        // What we CAN test: the function doesn't panic and
        // returns a non-empty string.
        let md = list_recordings_markdown();
        assert!(md.contains("# Corpus"), "got: {}", md);
    }

    #[test]
    fn parse_sidecar_strips_header_and_extracts_grammar() {
        // headerless (old recording) → no grammar, full body
        let (g, b) = parse_sidecar("just the paragraph\nsecond line");
        assert_eq!(g, "");
        assert_eq!(b, "just the paragraph\nsecond line");
        // full header → grammar extracted, header stripped
        let (g, b) = parse_sidecar("URL: u\nTITLE: t\nGRAMMAR: editor:helix\n---\nopen file");
        assert_eq!(g, "editor:helix");
        assert_eq!(b, "open file");
        // grammar-only header (shell recording, no page source)
        let (g, b) = parse_sidecar("GRAMMAR: shell\n---\ndelete word");
        assert_eq!(g, "shell");
        assert_eq!(b, "delete word");
        // headered but pre-grammar (v0.1.325 era) → header stripped, grammar empty
        let (g, b) = parse_sidecar("URL: u\nTITLE: t\n---\nbody text");
        assert_eq!(g, "");
        assert_eq!(b, "body text");
        // a body line containing GRAMMAR: mid-text is not re-parsed as header
        let (g, b) = parse_sidecar("GRAMMAR: web\n---\nNote: GRAMMAR: stays in body");
        assert_eq!(g, "web");
        assert_eq!(b, "Note: GRAMMAR: stays in body");
        // empty content
        let (g, b) = parse_sidecar("");
        assert_eq!(g, "");
        assert_eq!(b, "");
    }
}

// ===== v0.1.230: corpus listing for QML surfacing =====

/// A single corpus recording's metadata, for surfacing to the
/// user. Read from disk via `list_recordings`. We don't keep
/// these in memory — they're recomputed on demand because the
/// corpus directory is the source of truth and the UI is shown
/// on user navigation only.
#[derive(Debug, Clone)]
pub struct CorpusEntry {
    pub speaker_id: String,
    pub unix_ms: u64,
    /// Audio duration in seconds. None if we couldn't read the
    /// WAV header (corrupt file, partial write, foreign format).
    pub duration_s: Option<f32>,
    pub sample_rate: Option<u32>,
    /// First ~200 chars of the paragraph text. Truncated for
    /// display; full text remains on disk.
    pub paragraph_text: String,
    /// v0.1.511: the active voice-grammar label when recorded
    /// (e.g. "editor:helix", "repl:gdb", "shell", "web"), parsed from
    /// the sidecar header. Empty for recordings made before grammar
    /// stamping (v0.1.510) or with no header. Lets the listing be
    /// grouped/queried by grammar ("show my helix voice commands").
    pub grammar: String,
}

/// Scan the corpus directory and return all (speaker, recording)
/// pairs sorted by timestamp descending (newest first). Each
/// recording is identified by its stem (the unix_ms part of the
/// filename); the actual audio extension may be `.wav` or
/// `.opus` (v0.1.231) — we accept either, preferring opus when
/// both exist (the v0.1.231 conversion path would have deleted
/// the wav, but if conversion failed both are on disk; opus is
/// the canonical winner).
///
/// Returns empty Vec if the corpus dir doesn't exist (no
/// recordings yet) or HOME isn't set.
pub fn list_recordings() -> Vec<CorpusEntry> {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return Vec::new(),
    };
    let corpus_dir = PathBuf::from(home).join(".config/web2qml/corpus");
    if !corpus_dir.exists() { return Vec::new(); }
    let mut entries: Vec<CorpusEntry> = Vec::new();
    let speaker_dirs = match std::fs::read_dir(&corpus_dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    for speaker_entry in speaker_dirs.flatten() {
        let speaker_path = speaker_entry.path();
        if !speaker_path.is_dir() { continue; }
        let speaker_id = speaker_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        // Collect stems we've seen in this speaker dir so the
        // same recording isn't listed twice (once for .wav once
        // for .opus when both exist temporarily).
        let mut seen_stems: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // First pass: prefer .opus. Second pass: .wav for any
        // stem not already covered. Two reads of the dir keep
        // the logic simple.
        for pass in &["opus", "wav"] {
            let entries_iter = match std::fs::read_dir(&speaker_path) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            for f in entries_iter.flatten() {
                let path = f.path();
                if path.extension().and_then(|e| e.to_str()) != Some(*pass) {
                    continue;
                }
                let stem = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                if seen_stems.contains(&stem) { continue; }
                let unix_ms: u64 = match stem.parse() {
                    Ok(n) => n,
                    Err(_) => continue,
                };
                seen_stems.insert(stem);
                let (sample_rate, duration_s) = read_audio_metadata(&path, pass);
                let txt_path = path.with_extension("txt");
                // v0.1.511: strip the KEY: header (URL/TITLE/GRAMMAR/
                // ---) so it doesn't bleed into the displayed paragraph
                // text, and extract the grammar label for grouping.
                let raw = std::fs::read_to_string(&txt_path).unwrap_or_default();
                let (grammar, body) = parse_sidecar(&raw);
                let paragraph_text = body.chars().take(200).collect::<String>();
                entries.push(CorpusEntry {
                    speaker_id: speaker_id.clone(),
                    unix_ms,
                    duration_s,
                    sample_rate,
                    paragraph_text,
                    grammar,
                });
            }
        }
    }
    entries.sort_by(|a, b| b.unix_ms.cmp(&a.unix_ms));
    entries
}

/// Read audio metadata (sample rate + duration) from a wav or
/// opus file. For wav, parses the canonical 44-byte header.
/// For opus, estimates duration from file size assuming the
/// 32kbps target bitrate that `convert_wav_to_opus` uses —
/// inexact (constant-bitrate-ish, but opus is VBR) but close
/// enough for the corpus listing's "12.4s" annotation. Returns
/// (None, None) when we can't read.
fn read_audio_metadata(path: &PathBuf, ext: &str) -> (Option<u32>, Option<f32>) {
    match ext {
        "wav" => {
            let bytes = match std::fs::read(path) {
                Ok(b) => b,
                Err(_) => return (None, None),
            };
            if bytes.len() < 44 || &bytes[0..4] != b"RIFF" {
                return (None, None);
            }
            let sr = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
            let data_sz = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]);
            let dur = if sr > 0 {
                Some(data_sz as f32 / (sr as f32 * 2.0))
            } else { None };
            (Some(sr), dur)
        }
        "opus" => {
            // Estimate duration from file size and the encoder's
            // target bitrate (32kbps from convert_wav_to_opus).
            // Real duration could be ±20% off because opus is
            // VBR — but the listing only needs to convey
            // approximate length, not exact.
            //
            // bits = bytes * 8
            // seconds ≈ bits / bitrate_bps
            // bitrate_bps = 32_000
            let size = match std::fs::metadata(path) {
                Ok(m) => m.len(),
                Err(_) => return (None, None),
            };
            let bits = size as f32 * 8.0;
            // Subtract ~3KB for OGG container overhead (page
            // headers, opus_head, opus_tags). Approximate.
            let payload_bits = (bits - 24_000.0).max(0.0);
            let dur = payload_bits / 32_000.0;
            // Sample rate is fixed to whatever ffmpeg used. We
            // don't parse the OGG headers; return None for the
            // rate.
            (None, Some(dur))
        }
        _ => (None, None),
    }
}

/// Generate a markdown document showing corpus contents, ready
/// for the manpage rendering pipeline. Layout:
///
///   # Corpus
///   N recordings totalling ~M minutes across K speakers
///
///   ## <speaker_id>
///   J recordings
///
///   | When | Duration | Paragraph |
///   |------|----------|-----------|
///   | <human time> | <s> | <truncated text> |
///   ...
///
/// Groups by speaker, newest recordings first within each group.
/// Speakers ordered by total-recording-count descending so the
/// most-active speaker is at the top.
pub fn list_recordings_markdown() -> String {
    let entries = list_recordings();
    if entries.is_empty() {
        return "# Corpus\n\nNo recordings yet.\n\nRecordings accumulate when you read paragraphs aloud — paragraph select arms the recorder, and the captured audio plus the paragraph text are saved under `~/.config/web2qml/corpus/<speaker>/<timestamp>.{wav,txt}`.\n".into();
    }
    // Aggregate stats.
    let total = entries.len();
    let total_seconds: f32 = entries.iter()
        .filter_map(|e| e.duration_s)
        .sum();
    let mut speakers: std::collections::HashMap<String, Vec<&CorpusEntry>> =
        std::collections::HashMap::new();
    for e in &entries {
        speakers.entry(e.speaker_id.clone()).or_default().push(e);
    }
    // Speakers sorted by recording count desc, then name asc as
    // tiebreak for stable display.
    let mut speaker_list: Vec<(String, Vec<&CorpusEntry>)> = speakers.into_iter().collect();
    speaker_list.sort_by(|a, b| {
        b.1.len().cmp(&a.1.len())
            .then_with(|| a.0.cmp(&b.0))
    });

    let mut md = String::with_capacity(4096);
    md.push_str("# Corpus\n\n");
    let minutes = total_seconds / 60.0;
    md.push_str(&format!(
        "{} recording{} totalling ~{:.1} minute{} across {} speaker{}.\n\n",
        total, if total == 1 { "" } else { "s" },
        minutes, if (minutes - 1.0).abs() < 0.05 { "" } else { "s" },
        speaker_list.len(), if speaker_list.len() == 1 { "" } else { "s" }
    ));
    md.push_str("Recordings are stored under `~/.config/web2qml/corpus/`. \
                 Each `.wav` is 16-bit PCM mono with its source paragraph \
                 in a sibling `.txt`. Newest first per speaker.\n\n");

    for (speaker_id, recs) in &speaker_list {
        md.push_str("## ");
        md.push_str(speaker_id);
        md.push_str("\n\n");
        md.push_str(&format!("{} recording{}\n\n",
            recs.len(), if recs.len() == 1 { "" } else { "s" }));
        md.push_str("| When | Duration | Grammar | Paragraph |\n");
        md.push_str("|------|----------|---------|-----------|\n");
        for r in recs {
            let when = format_unix_ms(r.unix_ms);
            let dur = match r.duration_s {
                Some(s) => format!("{:.1}s", s),
                None => "?".to_string(),
            };
            // Escape pipes in paragraph text so the cell doesn't
            // break the table. Also truncate harder for table
            // display (60 chars; full 200 char text is in the
            // file on disk).
            //
            // v0.1.231: also strip control characters. The .txt
            // file was written directly from the user-selected
            // paragraph text, which can contain control chars
            // from upstream sources (CR-LF, tab, NUL, vertical
            // tab, ANSI escape sequences from a wonky webpage,
            // bidi-override marks). Those render as garbage
            // glyphs in the markdown table cell. Filter to
            // printable + space.
            let para_disp = {
                let trunc_count = 60;
                let truncated: String = r.paragraph_text.chars().take(trunc_count).collect();
                let mut cleaned = String::with_capacity(truncated.len());
                for c in truncated.chars() {
                    if c == '\n' || c == '\r' || c == '\t' {
                        cleaned.push(' ');
                    } else if c.is_control() {
                        // Drop other control chars entirely (NUL,
                        // bell, vertical-tab, etc).
                        continue;
                    } else if c == '\u{200E}' || c == '\u{200F}'
                        || c == '\u{202A}' || c == '\u{202B}'
                        || c == '\u{202C}' || c == '\u{202D}'
                        || c == '\u{202E}' || c == '\u{2066}'
                        || c == '\u{2067}' || c == '\u{2068}'
                        || c == '\u{2069}'
                    {
                        // Bidi/directional formatting marks. Can
                        // reverse following text rendering — drop.
                        continue;
                    } else {
                        cleaned.push(c);
                    }
                }
                if r.paragraph_text.chars().count() > trunc_count {
                    cleaned.push('…');
                }
                // Pipe escape — v0.1.229 split_md_row handles
                // this and the inline parser's \| → | rule keeps
                // it intact in the rendered cell.
                cleaned.replace('|', "\\|")
            };
            // v0.1.511: grammar label (parsed from the sidecar
            // header). Empty for pre-stamping recordings → show a dash.
            let gram_disp = if r.grammar.is_empty() {
                "—".to_string()
            } else {
                r.grammar.replace('|', "\\|")
            };
            md.push_str(&format!("| {} | {} | {} | {} |\n", when, dur, gram_disp, para_disp));
        }
        md.push('\n');
    }
    md
}

/// Format a unix-ms timestamp as a human-readable string. We
/// don't pull in chrono just for this — the std time library
/// gives us "seconds since epoch" but no calendar conversion.
/// Roll a minimal "Nh ago" / "Nm ago" / "Ns ago" relative format
/// using SystemTime::now() as the reference.
fn format_unix_ms(unix_ms: u64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(unix_ms);
    if unix_ms > now_ms {
        // Future timestamp — clock skew. Just emit raw seconds.
        return format!("ts={}", unix_ms);
    }
    let delta_s = (now_ms - unix_ms) / 1000;
    if delta_s < 60 {
        format!("{}s ago", delta_s)
    } else if delta_s < 3600 {
        format!("{}m ago", delta_s / 60)
    } else if delta_s < 86400 {
        format!("{}h ago", delta_s / 3600)
    } else {
        format!("{}d ago", delta_s / 86400)
    }
}
