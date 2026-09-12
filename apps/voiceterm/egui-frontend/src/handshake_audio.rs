//! handshake_audio — connects the acoustic handshake
//! (`peer_handshake::audio_dialup`, pure symbol↔sample encode/decode) to a
//! real microphone and speaker. Closes the gap `TODO_multi_device_share.md`
//! names as the last one: "audio_dialup's symbols never reach a speaker or
//! a microphone — this build has no live audio pipeline."
//!
//! ## What this ports, and what it deliberately does not
//!
//! `apps/voiceterm/src/voice.rs`'s `ensure_capture_running` is the
//! reference for capture, and it's real, hardened, cpal-based device
//! negotiation with a documented real-world failure history — v0.1.136's
//! own comment there: a bad default device used to disable voice for the
//! whole session, fixed by trying up to 3 candidate devices 250ms apart,
//! falling back through each device's supported configs if its default
//! fails. That retry/fallback shape earned itself from real bug reports;
//! it's ported here close to verbatim rather than rewritten from scratch.
//!
//! What ISN'T ported: that same reference function also feeds
//! `BG_RING_BUFFER` (an always-on background stream for voice-command
//! fingerprinting) and `corpus_recorder` (training-data capture) from the
//! SAME callback. Those are two separate, larger features this module has
//! no reason to depend on or stub out just to reach a microphone. The
//! handshake doesn't need a perpetual global stream either — it needs to
//! listen for one acoustic frame for a few seconds after the user says
//! "listen". So capture here is bounded and one-shot: open a stream,
//! collect up to `max_ms` of audio, close it. No `OnceLock`, no ring
//! buffer shared with anything else in the app.
//!
//! `play_modem`/the cpal output path, `resample_linear`, and the
//! `play_via_system`/WAV-file fallback ARE ported close to verbatim from
//! `apps/voiceterm/src/audio_feedback.rs` — that side was already
//! self-contained in the reference (no ring-buffer or cross-feature
//! coupling), so there was nothing to narrow.
//!
//! ## Verification ceiling
//!
//! Same honest limit as `bt_send.rs` and `bt-kit`'s `rfcomm_io.rs`: none of
//! this can be exercised against a real microphone/speaker in a build
//! sandbox. It's checked against the real `cpal` API (the same crate and
//! version voiceterm's QML side already depends on — see that crate's own
//! `Cargo.toml`) and the pure encode/decode/CRC logic it feeds is already
//! fully tested in `shared/peer-handshake`. The actual capture-and-decode
//! round trip against real room acoustics is not verified here.

use peer_handshake::audio_dialup::SAMPLE_RATE;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Plays PCM samples (any rate) out the default output device, resampling
/// to whatever the device actually wants first. Spawns its own thread —
/// callers (the handshake state machine driving a `Dialing` state) don't
/// block on playback finishing.
pub fn play_modem(pcm: Vec<f32>, rate: u32) {
    std::thread::spawn(move || {
        let resampled = resample_linear(&pcm, rate, output_device_rate().unwrap_or(rate));
        if play_cpal(&resampled).is_err() {
            let _ = play_via_system(&resampled, rate);
        }
    });
}

fn output_device_rate() -> Option<u32> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let device = cpal::default_host().default_output_device()?;
    Some(device.default_output_config().ok()?.sample_rate().0)
}

fn play_cpal(pcm: &[f32]) -> Result<(), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no output device")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let channels = supported.channels() as usize;
    let format = supported.sample_format();

    let pos = Arc::new(AtomicUsize::new(0));
    let pcm_arc = Arc::new(pcm.to_vec());
    let done = Arc::new(Mutex::new(false));
    let err_fn = |err| eprintln!("handshake_audio: output stream error: {err}");

    let stream = match format {
        cpal::SampleFormat::F32 => {
            let (pos, pcm, done) = (pos.clone(), pcm_arc.clone(), done.clone());
            device
                .build_output_stream(
                    &supported.config(),
                    move |data: &mut [f32], _: &_| fill_frame(data, channels, &pos, &pcm, &done, |s| s),
                    err_fn,
                    None,
                )
                .map_err(|e| e.to_string())?
        }
        cpal::SampleFormat::I16 => {
            let (pos, pcm, done) = (pos.clone(), pcm_arc.clone(), done.clone());
            device
                .build_output_stream(
                    &supported.config(),
                    move |data: &mut [i16], _: &_| {
                        fill_frame(data, channels, &pos, &pcm, &done, |s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                    },
                    err_fn,
                    None,
                )
                .map_err(|e| e.to_string())?
        }
        other => return Err(format!("unsupported output format: {other:?}")),
    };

    stream.play().map_err(|e| e.to_string())?;

    // The modem frame is short (audio_dialup's own numbers: on the order of
    // a few seconds for a whole handshake payload) — 5s is generous
    // headroom, not a guess at exactly how long any given payload takes.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if *done.lock().unwrap() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

/// Shared frame-fill logic for both output sample formats: walk `pcm` at
/// `pos`, converting each sample with `conv`, marking `done` once past the
/// end. Pulled out once rather than written twice (F32 and I16 bodies were
/// identical but for the conversion) — a fix to one that isn't made to the
/// other is exactly how these two formats drift apart over time.
fn fill_frame<T: Copy + Default>(
    data: &mut [T],
    channels: usize,
    pos: &AtomicUsize,
    pcm: &[f32],
    done: &Mutex<bool>,
    conv: impl Fn(f32) -> T,
) {
    for frame in data.chunks_mut(channels.max(1)) {
        let i = pos.fetch_add(1, Ordering::SeqCst);
        let sample = if i < pcm.len() {
            conv(pcm[i])
        } else {
            if let Ok(mut d) = done.try_lock() {
                *d = true;
            }
            T::default()
        };
        for x in frame.iter_mut() {
            *x = sample;
        }
    }
}

fn resample_linear(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = (input.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let idx = src as usize;
        if idx >= input.len() {
            break;
        }
        let frac = src - idx as f64;
        let next = (idx + 1).min(input.len() - 1);
        out.push((input[idx] as f64 * (1.0 - frac) + input[next] as f64 * frac) as f32);
    }
    out
}

/// Fallback when cpal can't open an output stream at all: write a WAV file
/// and shell out to whatever's actually on the system. Tries in order of
/// how likely each is to exist on a target device (PipeWire/PulseAudio
/// first, since that's this fleet's actual audio stack per the reference's
/// own comment; ALSA and macOS's `afplay` as further fallbacks, not the
/// expected path).
fn play_via_system(pcm: &[f32], rate: u32) -> Result<(), String> {
    let wav_path = std::env::temp_dir().join(format!("handshake_tone_{}.wav", std::process::id()));
    write_wav(&wav_path, pcm, rate).map_err(|e| e.to_string())?;
    let result = ["pw-play", "paplay", "aplay", "afplay"].iter().find_map(|cmd| {
        std::process::Command::new(cmd)
            .arg(&wav_path)
            .stderr(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
            .ok()
            .filter(|s| s.success())
    });
    let _ = std::fs::remove_file(&wav_path);
    result.map(|_| ()).ok_or_else(|| "no audio player available".to_string())
}

fn write_wav(path: &std::path::Path, pcm: &[f32], sample_rate: u32) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    let bytes_per_sample = 2u32; // i16 PCM
    let byte_rate = sample_rate * bytes_per_sample;
    let data_size = pcm.len() as u32 * bytes_per_sample;

    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_size).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&1u16.to_le_bytes())?; // mono
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&(bytes_per_sample as u16).to_le_bytes())?; // block align
    f.write_all(&16u16.to_le_bytes())?; // bits per sample
    f.write_all(b"data")?;
    f.write_all(&data_size.to_le_bytes())?;
    for &s in pcm {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        f.write_all(&v.to_le_bytes())?;
    }
    Ok(())
}

/// One-shot capture: opens an input stream (same hardened device-selection
/// as the reference — default first, then every enumerated device, with a
/// default-config attempt then a scan of supported configs, retried up to
/// 3 times 250ms apart to ride out a PipeWire hiccup right after launch),
/// collects up to `max_ms` of audio at whatever rate the device actually
/// runs, then stops the stream and returns the samples resampled to
/// `peer_handshake::audio_dialup::SAMPLE_RATE`.
///
/// Blocks the calling thread for up to `max_ms` — callers on the handshake
/// UI thread should call this from a spawned thread, the same way
/// `bt_send.rs` isolates its own blocking work rather than running it on
/// the render thread.
pub fn capture_modem_samples(max_ms: u32) -> Result<Vec<f32>, String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let mut candidates: Vec<cpal::Device> = Vec::new();
    if let Some(d) = host.default_input_device() {
        candidates.push(d);
    }
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            let name = d.name().unwrap_or_default();
            if candidates.iter().any(|c| c.name().unwrap_or_default() == name) {
                continue;
            }
            candidates.push(d);
        }
    }
    if candidates.is_empty() {
        return Err("no input device available".into());
    }

    let mut chosen: Option<(cpal::Device, cpal::SupportedStreamConfig)> = None;
    'outer: for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(250));
        }
        for dev in &candidates {
            if let Ok(cfg) = dev.default_input_config() {
                chosen = Some((dev.clone(), cfg));
                break 'outer;
            }
            if let Ok(supported) = dev.supported_input_configs() {
                for range in supported {
                    if range.channels() == 0 || range.channels() > 8 {
                        continue;
                    }
                    if range.max_sample_rate().0 < 8000 {
                        continue;
                    }
                    let sr = range.max_sample_rate().0.min(48000).max(range.min_sample_rate().0);
                    chosen = Some((dev.clone(), range.with_sample_rate(cpal::SampleRate(sr))));
                    break 'outer;
                }
            }
        }
    }
    let (device, config) = chosen.ok_or("no working input config after 3 attempts")?;
    let device_rate = config.sample_rate().0;
    let format = config.sample_format();

    let collected: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let target_len = (device_rate as u64 * max_ms as u64 / 1000) as usize;
    let err_fn = |err| eprintln!("handshake_audio: input stream error: {err}");

    let stream = {
        let sink = collected.clone();
        match format {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &_| {
                    if let Ok(mut g) = sink.lock() {
                        g.extend_from_slice(data);
                    }
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &_| {
                    if let Ok(mut g) = sink.lock() {
                        g.extend(data.iter().map(|&s| s as f32 / 32768.0));
                    }
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &config.into(),
                move |data: &[u16], _: &_| {
                    if let Ok(mut g) = sink.lock() {
                        g.extend(data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0));
                    }
                },
                err_fn,
                None,
            ),
            other => return Err(format!("unsupported input format: {other:?}")),
        }
        .map_err(|e| e.to_string())?
    };

    stream.play().map_err(|e| e.to_string())?;
    let start = Instant::now();
    loop {
        if start.elapsed() >= Duration::from_millis(max_ms as u64) {
            break;
        }
        if collected.lock().map(|g| g.len() >= target_len).unwrap_or(false) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stream); // stops capture before we touch the buffer further

    let mono = collected.lock().map_err(|_| "capture buffer poisoned".to_string())?.clone();
    Ok(resample_linear(&mono, device_rate, SAMPLE_RATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_to_the_same_rate_is_a_no_op_copy() {
        let input = vec![0.1, 0.2, 0.3, 0.4];
        assert_eq!(resample_linear(&input, 8000, 8000), input);
    }

    #[test]
    fn resample_empty_input_stays_empty() {
        assert_eq!(resample_linear(&[], 48000, 8000), Vec::<f32>::new());
    }

    #[test]
    fn downsampling_produces_roughly_the_expected_length() {
        // 48kHz -> 8kHz is a 6x reduction. Exact length depends on the
        // linear-interpolation edge handling, so this checks "close", not
        // "exact" -- an off-by-one here is not a correctness bug worth a
        // brittle exact-equality test.
        let input = vec![0.0f32; 48_000];
        let out = resample_linear(&input, 48_000, 8_000);
        assert!((out.len() as i64 - 8_000).abs() < 10, "got {} samples, expected ~8000", out.len());
    }

    #[test]
    fn a_wav_file_round_trips_a_readable_header() {
        // Not a full WAV parser -- just confirms the header fields land at
        // the byte offsets a real player expects, since a subtly wrong
        // header is the kind of bug that "plays as silence" rather than
        // erroring, and would be easy to ship unnoticed.
        let dir = std::env::temp_dir().join(format!("handshake-audio-wav-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.wav");
        write_wav(&path, &[0.5, -0.5, 0.0], 8000).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 8000, "sample rate at the documented offset");
        assert_eq!(&bytes[36..40], b"data");
        let data_size = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(data_size, 6, "3 i16 samples = 6 bytes");
        std::fs::remove_dir_all(&dir).ok();
    }
}
