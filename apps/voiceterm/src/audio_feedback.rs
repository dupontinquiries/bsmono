// src/audio_feedback.rs
// Plays short synthesized tones for voice UI state transitions.
// Tones are generated in-memory and played via cpal (if voice feature)
// or piped through `paplay`/`aplay` as a fallback.
//
// Tones (all under 200ms):
//   start  — rising two-tone (440Hz -> 880Hz, 80ms each)
//   stop   — falling two-tone (880Hz -> 440Hz, 60ms each)
//   ok     — single short blip (660Hz, 70ms) for successful intent
//   error  — descending dip (300Hz -> 200Hz, 100ms)

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

static PLAYING: AtomicBool = AtomicBool::new(false);

const SAMPLE_RATE: u32 = 22050;

/// Tone shape: (frequency_hz, duration_ms)
type ToneSegment = (f32, u32);

#[derive(Clone, Copy, Debug)]
pub enum FeedbackTone {
    Start,
    Stop,
    Ok,
    Error,
}

impl FeedbackTone {
    fn segments(&self) -> &'static [ToneSegment] {
        match self {
            FeedbackTone::Start => &[(440.0, 60), (880.0, 80)],
            FeedbackTone::Stop  => &[(880.0, 50), (440.0, 60)],
            FeedbackTone::Ok    => &[(660.0, 70)],
            FeedbackTone::Error => &[(300.0, 70), (200.0, 90)],
        }
    }
}

/// Play tone in a background thread (non-blocking).
/// Multiple rapid calls are deduplicated — if one is playing, others are dropped.
pub fn play(tone: FeedbackTone) {
    if PLAYING.swap(true, Ordering::SeqCst) {
        // Already playing — skip to avoid overlap
        return;
    }
    thread::spawn(move || {
        play_blocking(tone);
        PLAYING.store(false, Ordering::SeqCst);
    });
}

/// v0.1.712: play raw modem PCM (the acoustic dial-up handshake tones).
/// `pcm` is mono f32 at `rate` Hz; resampled to the feedback rate then played
/// via the same cpal/system path as the UI tones. Spawns a thread; best-effort.
/// RUNTIME-UNTESTED in-sandbox (no audio device) — verified only by compile.
#[cfg(feature = "voice")]
pub fn play_modem(pcm: Vec<f32>, rate: u32) {
    thread::spawn(move || {
        let at_fb = resample_linear(&pcm, rate, SAMPLE_RATE);
        if play_cpal(&at_fb).is_err() {
            let _ = play_via_system(&at_fb);
        }
    });
}

/// No-op when the voice/audio feature is off (e.g. the pure test harness).
#[cfg(not(feature = "voice"))]
pub fn play_modem(_pcm: Vec<f32>, _rate: u32) {}

fn play_blocking(tone: FeedbackTone) {
    let pcm = synthesize(tone);
    // Try cpal first (only if voice feature)
    #[cfg(feature = "voice")]
    {
        if play_cpal(&pcm).is_ok() { return; }
    }
    // Fallback: write WAV to temp file and play with system tool
    let _ = play_via_system(&pcm);
}

/// Synthesize tone into f32 PCM samples
fn synthesize(tone: FeedbackTone) -> Vec<f32> {
    let segments = tone.segments();
    let total_samples: usize = segments.iter()
        .map(|(_, ms)| (*ms as usize * SAMPLE_RATE as usize) / 1000)
        .sum();
    let mut pcm = Vec::with_capacity(total_samples);

    for &(freq, ms) in segments {
        let n = (ms as usize * SAMPLE_RATE as usize) / 1000;
        // Soft envelope: fade in/out over 5ms each end to avoid clicks
        let fade = (SAMPLE_RATE as usize / 200).min(n / 4);
        for i in 0..n {
            let t = i as f32 / SAMPLE_RATE as f32;
            // Sine wave with slight harmonic for richer sound
            let sample = (2.0 * std::f32::consts::PI * freq * t).sin() * 0.4
                       + (4.0 * std::f32::consts::PI * freq * t).sin() * 0.08;
            // Envelope
            let env = if i < fade { i as f32 / fade as f32 }
                      else if i > n - fade { (n - i) as f32 / fade as f32 }
                      else { 1.0 };
            pcm.push(sample * env * 0.35); // overall volume
        }
    }
    pcm
}

#[cfg(feature = "voice")]
fn play_cpal(pcm: &[f32]) -> Result<(), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::{Arc, Mutex};
    use std::sync::atomic::AtomicUsize;

    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("No output device")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let device_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let format = supported.sample_format();

    // Resample our 22050Hz pcm to device rate if different
    let resampled = if device_rate == SAMPLE_RATE { pcm.to_vec() } else {
        resample_linear(pcm, SAMPLE_RATE, device_rate)
    };

    let pos = Arc::new(AtomicUsize::new(0));
    let pcm_arc = Arc::new(resampled);
    let done = Arc::new(Mutex::new(false));
    let pos_clone = pos.clone();
    let pcm_clone = pcm_arc.clone();
    let done_clone = done.clone();

    let err_fn = |err| eprintln!("Audio output error: {}", err);

    let stream = match format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            &supported.config(),
            move |data: &mut [f32], _: &_| {
                let pcm = &*pcm_clone;
                for frame in data.chunks_mut(channels) {
                    let i = pos_clone.fetch_add(1, Ordering::SeqCst);
                    let s = if i < pcm.len() { pcm[i] } else {
                        if let Ok(mut d) = done_clone.try_lock() { *d = true; }
                        0.0
                    };
                    for x in frame.iter_mut() { *x = s; }
                }
            },
            err_fn, None
        ).map_err(|e| e.to_string())?,
        cpal::SampleFormat::I16 => device.build_output_stream(
            &supported.config(),
            move |data: &mut [i16], _: &_| {
                let pcm = &*pcm_clone;
                for frame in data.chunks_mut(channels) {
                    let i = pos_clone.fetch_add(1, Ordering::SeqCst);
                    let s = if i < pcm.len() { (pcm[i].clamp(-1.0, 1.0) * 32767.0) as i16 } else {
                        if let Ok(mut d) = done_clone.try_lock() { *d = true; }
                        0
                    };
                    for x in frame.iter_mut() { *x = s; }
                }
            },
            err_fn, None
        ).map_err(|e| e.to_string())?,
        _ => return Err(format!("Unsupported format: {:?}", format)),
    };

    stream.play().map_err(|e| e.to_string())?;

    // Wait for playback to finish (max 1 second)
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(1) {
        if *done.lock().unwrap() { break; }
        thread::sleep(std::time::Duration::from_millis(20));
    }
    drop(stream);
    Ok(())
}

#[cfg(feature = "voice")]
fn resample_linear(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to { return input.to_vec(); }
    let ratio = from as f64 / to as f64;
    let out_len = (input.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let idx = src as usize;
        if idx >= input.len() { break; }
        let frac = src - idx as f64;
        let next = (idx + 1).min(input.len() - 1);
        out.push((input[idx] as f64 * (1.0 - frac) + input[next] as f64 * frac) as f32);
    }
    out
}

/// Fallback: write WAV and play via system audio tool
fn play_via_system(pcm: &[f32]) -> Result<(), String> {
    let wav_path = "/tmp/web2qml_tone.wav";
    write_wav(wav_path, pcm, SAMPLE_RATE).map_err(|e| e.to_string())?;

    // Try paplay (PulseAudio/PipeWire), aplay (ALSA), afplay (macOS)
    for cmd in &["paplay", "pw-play", "aplay", "afplay"] {
        if let Ok(status) = std::process::Command::new(cmd)
            .arg(wav_path)
            .stderr(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
        {
            if status.success() { let _ = std::fs::remove_file(wav_path); return Ok(()); }
        }
    }
    let _ = std::fs::remove_file(wav_path);
    Err("No audio player available".into())
}

/// Minimal WAV writer for f32 mono PCM
fn write_wav(path: &str, pcm: &[f32], sample_rate: u32) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    let bytes_per_sample = 2u16; // i16 PCM
    let byte_rate = sample_rate * bytes_per_sample as u32;
    let data_size = pcm.len() as u32 * bytes_per_sample as u32;

    // RIFF header
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_size).to_le_bytes())?;
    f.write_all(b"WAVE")?;
    // fmt chunk
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;        // PCM
    f.write_all(&1u16.to_le_bytes())?;        // mono
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&bytes_per_sample.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;       // bits per sample
    // data chunk
    f.write_all(b"data")?;
    f.write_all(&data_size.to_le_bytes())?;
    for &s in pcm {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        f.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each FeedbackTone variant must return a non-empty segments
    /// slice; otherwise synthesize() would yield silence.
    #[test]
    fn every_tone_has_segments() {
        for tone in [FeedbackTone::Start, FeedbackTone::Stop,
                     FeedbackTone::Ok, FeedbackTone::Error] {
            let segs = tone.segments();
            assert!(!segs.is_empty(), "{:?} has no segments", tone);
            for (freq, ms) in segs {
                assert!(*freq > 0.0, "{:?} has non-positive freq", tone);
                assert!(*ms > 0, "{:?} has zero duration", tone);
            }
        }
    }

    /// All tones must finish in well under a second so the
    /// non-blocking `play()` doesn't accidentally serialize a
    /// long queue. The comment in the file ("all under 200ms")
    /// is a soft spec; we lock it to 250ms as the hard cap.
    #[test]
    fn every_tone_under_250ms() {
        for tone in [FeedbackTone::Start, FeedbackTone::Stop,
                     FeedbackTone::Ok, FeedbackTone::Error] {
            let total_ms: u32 = tone.segments().iter()
                .map(|(_, ms)| *ms).sum();
            assert!(total_ms <= 250,
                "{:?} is {}ms — exceeds 250ms cap", tone, total_ms);
        }
    }

    /// Synthesized PCM length should match the sum of segment
    /// durations, scaled by the sample rate. Off-by-one tolerance
    /// because each segment rounds independently.
    #[test]
    fn synthesize_yields_expected_sample_count() {
        for tone in [FeedbackTone::Start, FeedbackTone::Stop,
                     FeedbackTone::Ok, FeedbackTone::Error] {
            let pcm = synthesize(tone);
            let expected: usize = tone.segments().iter()
                .map(|(_, ms)| (*ms as usize * SAMPLE_RATE as usize) / 1000)
                .sum();
            assert_eq!(pcm.len(), expected,
                "{:?} sample count: got {}, expected {}",
                tone, pcm.len(), expected);
        }
    }

    /// Every sample must be in [-1.0, 1.0]. Out-of-range samples
    /// would clip on the i16 path in write_wav and produce audible
    /// distortion. The synth applies a 0.35 overall volume and an
    /// envelope so the worst case is well below 1.0; tests pin
    /// the bound so any future amplitude change doesn't accidentally
    /// blow it.
    #[test]
    fn synthesize_samples_in_unit_range() {
        for tone in [FeedbackTone::Start, FeedbackTone::Stop,
                     FeedbackTone::Ok, FeedbackTone::Error] {
            let pcm = synthesize(tone);
            for (i, s) in pcm.iter().enumerate() {
                assert!(s.is_finite(), "{:?}: NaN/inf at sample {}", tone, i);
                assert!(*s >= -1.0 && *s <= 1.0,
                    "{:?}: sample {} = {} out of [-1, 1]", tone, i, s);
            }
        }
    }

    /// The fade envelope means the first and last samples of each
    /// tone should be near zero (avoiding the click that a hard
    /// edge would produce). Each tone's PCM must start at very
    /// low amplitude.
    #[test]
    fn synthesize_first_sample_near_zero() {
        // Every tone's first sample must be near 0 (fade-in).
        for tone in [FeedbackTone::Start, FeedbackTone::Stop,
                     FeedbackTone::Ok, FeedbackTone::Error] {
            let pcm = synthesize(tone);
            assert!(pcm[0].abs() < 0.01,
                "{:?}: first sample {} not near zero", tone, pcm[0]);
        }
    }

    /// The last sample of the last segment must also fade to near
    /// zero. Same anti-click rationale as the first-sample test.
    #[test]
    fn synthesize_last_sample_near_zero() {
        for tone in [FeedbackTone::Start, FeedbackTone::Stop,
                     FeedbackTone::Ok, FeedbackTone::Error] {
            let pcm = synthesize(tone);
            let last = *pcm.last().expect("non-empty");
            assert!(last.abs() < 0.01,
                "{:?}: last sample {} not near zero", tone, last);
        }
    }

    /// Synthesis is deterministic — same tone produces identical
    /// PCM each call. Sanity check for the math; if some env-var
    /// or RNG ever sneaks in, this test catches it.
    #[test]
    fn synthesize_is_deterministic() {
        let a = synthesize(FeedbackTone::Start);
        let b = synthesize(FeedbackTone::Start);
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(x.to_bits(), y.to_bits(),
                "sample {} differs: {} vs {}", i, x, y);
        }
    }

    /// WAV header is exactly 44 bytes for the format we emit
    /// (PCM mono 16-bit, no chunk extensions). data_size is
    /// `samples * 2` bytes. Total file size: 44 + data_size.
    #[test]
    fn write_wav_produces_well_formed_file() {
        let pcm = synthesize(FeedbackTone::Ok);
        let path = format!("/tmp/audiotest_{}.wav", std::process::id());
        write_wav(&path, &pcm, SAMPLE_RATE).expect("write_wav");
        let bytes = std::fs::read(&path).expect("read");
        let _ = std::fs::remove_file(&path);

        // Total = 44 header + 2 bytes per sample.
        let expected_total = 44 + pcm.len() * 2;
        assert_eq!(bytes.len(), expected_total,
            "WAV file size: got {}, expected {}", bytes.len(), expected_total);

        // RIFF header.
        assert_eq!(&bytes[0..4], b"RIFF", "missing RIFF magic");
        // riff_size = total - 8 = 36 + data_size.
        let riff_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        assert_eq!(riff_size as usize, bytes.len() - 8,
            "riff_size field wrong");
        assert_eq!(&bytes[8..12], b"WAVE", "missing WAVE marker");

        // fmt chunk.
        assert_eq!(&bytes[12..16], b"fmt ", "missing fmt chunk");
        let fmt_size = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        assert_eq!(fmt_size, 16, "fmt chunk size should be 16");
        let audio_format = u16::from_le_bytes(bytes[20..22].try_into().unwrap());
        assert_eq!(audio_format, 1, "audio_format should be 1 (PCM)");
        let channels = u16::from_le_bytes(bytes[22..24].try_into().unwrap());
        assert_eq!(channels, 1, "channels should be 1 (mono)");
        let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        assert_eq!(rate, SAMPLE_RATE, "sample rate wrong");
        let bits_per_sample = u16::from_le_bytes(bytes[34..36].try_into().unwrap());
        assert_eq!(bits_per_sample, 16, "bits per sample should be 16");

        // data chunk.
        assert_eq!(&bytes[36..40], b"data", "missing data chunk");
        let data_size = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(data_size as usize, pcm.len() * 2, "data_size wrong");
    }

    /// Empty PCM should still produce a valid (but tiny) WAV.
    /// data_size = 0; total file size = 44.
    #[test]
    fn write_wav_handles_empty_pcm() {
        let path = format!("/tmp/audiotest_empty_{}.wav", std::process::id());
        write_wav(&path, &[], SAMPLE_RATE).expect("write_wav");
        let bytes = std::fs::read(&path).expect("read");
        let _ = std::fs::remove_file(&path);
        assert_eq!(bytes.len(), 44, "empty PCM should give 44-byte header-only WAV");
        let data_size = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(data_size, 0);
    }

    /// Out-of-range samples (>1.0 or <-1.0) get clamped to i16
    /// max/min during write_wav. Verify by writing a 3-sample PCM
    /// with extreme values and parsing the output back.
    #[test]
    fn write_wav_clamps_out_of_range_samples() {
        let pcm = vec![2.0f32, -2.0, 0.0];  // first will clip high, second low
        let path = format!("/tmp/audiotest_clip_{}.wav", std::process::id());
        write_wav(&path, &pcm, SAMPLE_RATE).expect("write_wav");
        let bytes = std::fs::read(&path).expect("read");
        let _ = std::fs::remove_file(&path);

        // The 3 i16 samples live at bytes 44..50.
        let s0 = i16::from_le_bytes(bytes[44..46].try_into().unwrap());
        let s1 = i16::from_le_bytes(bytes[46..48].try_into().unwrap());
        let s2 = i16::from_le_bytes(bytes[48..50].try_into().unwrap());
        // Note: clamp at 1.0 then * 32767 = 32767 exactly.
        // For -1.0: -1.0 * 32767 = -32767 (NOT -32768 — the
        // function loses one negative value to avoid the asymmetric-
        // range nuance, which is fine for tone playback).
        assert_eq!(s0, 32767, "+2.0 should clamp to i16 max");
        assert_eq!(s1, -32767, "-2.0 should clamp to -32767");
        assert_eq!(s2, 0, "0.0 should be 0");
    }

    /// Tones must be distinguishable. If two tone tables somehow
    /// converged to identical segments (refactor accident), the
    /// user couldn't tell start from stop.
    #[test]
    fn distinct_tones_produce_distinct_pcm() {
        let start = synthesize(FeedbackTone::Start);
        let stop  = synthesize(FeedbackTone::Stop);
        let ok    = synthesize(FeedbackTone::Ok);
        let err   = synthesize(FeedbackTone::Error);

        let pairs = [
            ("Start", &start, "Stop", &stop),
            ("Start", &start, "Ok", &ok),
            ("Start", &start, "Error", &err),
            ("Stop", &stop, "Ok", &ok),
            ("Stop", &stop, "Error", &err),
            ("Ok", &ok, "Error", &err),
        ];
        for (na, a, nb, b) in pairs {
            // Compare by length first (cheap), then by samples.
            // Two tones with different durations are trivially
            // distinct. Same-length ones must differ at some sample.
            if a.len() != b.len() { continue; }
            let differs = a.iter().zip(b.iter())
                .any(|(x, y)| x.to_bits() != y.to_bits());
            assert!(differs, "{} and {} produced identical PCM", na, nb);
        }
    }
}
