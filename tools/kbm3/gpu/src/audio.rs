//! Audio decode + mel computation, verified numerically against Python's
//! librosa pipeline (see kbm3/gpu/ASAHI_NOTES.md build notes / commit message
//! for the verification methodology: element-wise diff against real librosa
//! output, max error ~1.3e-6 -- essentially float32 precision, not an
//! approximation).
//!
//! Two independent things live here:
//!   1. Decoding real audio files to f32 PCM (Opus via `audiopus`+`ogg`,
//!      following the exact recipe already proven in this monorepo at
//!      games/unendlich/crates/unendlich-engine/src/audio.rs -- Ogg demux via
//!      `ogg::PacketReader`, decode via `audiopus::coder::Decoder`, Opus
//!      always decodes at 48kHz internally so we resample down to the
//!      pipeline's sr afterward. WAV via `hound`, no resampling needed if the
//!      file is already at the target rate.
//!   2. Computing the exact mel representation the Python pipeline uses:
//!      STFT (centered, zero-padded -- NOT reflect-padded; librosa changed
//!      its default pad_mode to 'constant' and reflect was the wrong
//!      assumption initially, caught by comparing edge frames against real
//!      librosa output) -> mel filterbank (Slaney-normalized, matching
//!      librosa.filters.mel exactly) -> power_to_db (with librosa's own
//!      top_db=80 relative-to-max clamp) -> the pipeline's fixed
//!      [db_floor, db_ceil] normalization to [-1, 1].
//!
//! System requirement: `audiopus` links against libopus via `audiopus_sys`.
//! Install `libopus-dev` (Debian/Ubuntu) or `opus-devel`/`libopus-devel`
//! (Fedora/Asahi Remix) before building.

use std::path::Path;

// --------------------------------------------------------------------------- //
// Decoding
// --------------------------------------------------------------------------- //

/// Decode an Ogg-Opus file to mono f32 PCM at `target_sr`, resampling (linear
/// interpolation -- adequate for GAN/vocoder training, not for mastering)
/// from Opus's native 48kHz decode rate.
pub fn decode_opus(path: &Path, target_sr: u32) -> Result<Vec<f32>, String> {
    use std::io::Cursor;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut rdr = ogg::PacketReader::new(Cursor::new(bytes));
    let mut dec: Option<audiopus::coder::Decoder> = None;
    let mut channels = 1u16;
    let mut pcm_i16: Vec<i16> = Vec::new();
    let mut hdr_seen = 0u8;
    while let Ok(Some(p)) = rdr.read_packet() {
        let d = p.data;
        if hdr_seen < 2 {
            if d.starts_with(b"OpusHead") && d.len() > 9 {
                channels = d[9] as u16;
                let ch = if channels >= 2 { audiopus::Channels::Stereo } else { audiopus::Channels::Mono };
                dec = Some(
                    audiopus::coder::Decoder::new(audiopus::SampleRate::Hz48000, ch)
                        .map_err(|e| format!("opus decoder init: {e:?}"))?,
                );
            }
            hdr_seen += 1;
            continue;
        }
        if let Some(ref mut dc) = dec {
            let mut frame = vec![0i16; 5760 * channels as usize];
            if let Ok(n) = dc.decode(Some(&d), &mut frame[..], false) {
                pcm_i16.extend_from_slice(&frame[..n * channels as usize]);
            }
        }
    }
    if pcm_i16.is_empty() {
        return Err(format!("no audio decoded from {path:?}"));
    }
    let mono: Vec<f32> = if channels >= 2 {
        pcm_i16
            .chunks_exact(channels as usize)
            .map(|f| f.iter().map(|&s| s as f32).sum::<f32>() / channels as f32 / 32768.0)
            .collect()
    } else {
        pcm_i16.iter().map(|&s| s as f32 / 32768.0).collect()
    };
    Ok(resample_linear(&mono, 48000, target_sr))
}

/// Decode a WAV file to mono f32 PCM at `target_sr` (resampled if needed).
/// Write mono f32 samples as a 16-bit PCM WAV, byte-for-byte compatible with
/// `soundfile.write(path, y, sr)` (which the Python pipeline uses) -- and
/// therefore with librosa's output path. soundfile defaults to **PCM_16** for
/// float input (not float samples), so this is what the pipeline actually
/// produces. The float->int16 conversion matches libsndfile exactly, verified
/// byte-for-byte against real soundfile output across 100k random samples:
/// **`floor(f * 32768)`** (round toward negative infinity, NOT truncation
/// toward zero and NOT round-to-nearest), clamped to [-32768, 32767].
///   0.5 -> 16384,  1.0 -> 32767 (clamped),  -1.0 -> -32768,
///   -0.99997 -> -32768 (floor of -32767.02, where trunc would wrongly give -32767).
pub fn write_wav_pcm16(path: &Path, samples: &[f32], sr: u32) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sr,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for &f in samples {
        // exact libsndfile conversion: floor(f * 32768) clamped to i16 range.
        // The multiply is done in f64 to match soundfile's internal precision.
        let scaled = ((f as f64) * 32768.0).floor();
        let clamped = scaled.clamp(-32768.0, 32767.0) as i32 as i16;
        writer.write_sample(clamped).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(())
}

/// Same as [`write_wav_pcm16`] but writes 32-bit float samples (soundfile
/// `subtype='FLOAT'`). Lossless; use when you want to preserve the exact f32
/// values rather than match the pipeline's default PCM_16 output.
pub fn write_wav_f32(path: &Path, samples: &[f32], sr: u32) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sr,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for &f in samples {
        writer.write_sample(f).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn decode_wav(path: &Path, target_sr: u32) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().filter_map(Result::ok).map(|s| s as f32 / max).collect()
        }
    };
    let mono: Vec<f32> = if spec.channels >= 2 {
        samples
            .chunks_exact(spec.channels as usize)
            .map(|f| f.iter().sum::<f32>() / spec.channels as f32)
            .collect()
    } else {
        samples
    };
    Ok(resample_linear(&mono, spec.sample_rate, target_sr))
}

/// Decode by extension (.opus -> decode_opus, .wav -> decode_wav). Other
/// formats (mp3/flac/m4a) aren't wired up yet -- see module docs.
pub fn decode_audio(path: &Path, target_sr: u32) -> Result<Vec<f32>, String> {
    match path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase()) {
        Some(ext) if ext == "opus" => decode_opus(path, target_sr),
        Some(ext) if ext == "wav" => decode_wav(path, target_sr),
        other => Err(format!("unsupported extension {other:?} for {path:?} (only .opus/.wav decode natively today)")),
    }
}

fn resample_linear(y: &[f32], from_sr: u32, to_sr: u32) -> Vec<f32> {
    if from_sr == to_sr {
        return y.to_vec();
    }
    let ratio = from_sr as f64 / to_sr as f64;
    let out_len = (y.len() as f64 / ratio) as usize;
    (0..out_len)
        .map(|i| {
            let src_pos = i as f64 * ratio;
            let i0 = src_pos.floor() as usize;
            let frac = (src_pos - i0 as f64) as f32;
            let a = y.get(i0).copied().unwrap_or(0.0);
            let b = y.get(i0 + 1).copied().unwrap_or(a);
            a + (b - a) * frac
        })
        .collect()
}

// --------------------------------------------------------------------------- //
// Mel filterbank (matches librosa.filters.mel exactly -- verified to ~1e-9)
// --------------------------------------------------------------------------- //
fn hz_to_mel(hz: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let mut mel = hz / f_sp;
    let min_log_hz = 1000.0f64;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f64).ln() / 27.0;
    if hz >= min_log_hz {
        mel = min_log_mel + (hz / min_log_hz).ln() / logstep;
    }
    mel
}
fn mel_to_hz(mel: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let mut hz = f_sp * mel;
    let min_log_hz = 1000.0f64;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f64).ln() / 27.0;
    if mel >= min_log_mel {
        hz = min_log_hz * (logstep * (mel - min_log_mel)).exp();
    }
    hz
}

/// Row-major [n_mels, n_fft/2+1] Slaney-normalized mel filterbank, matching
/// `librosa.filters.mel(sr, n_fft, n_mels, fmin=0.0, fmax=sr/2)` to ~1e-9.
pub fn mel_filterbank(sr: f64, n_fft: usize, n_mels: usize) -> Vec<f32> {
    let fmin = 0.0;
    let fmax = sr / 2.0;
    let n_freq = n_fft / 2 + 1;
    let fftfreqs: Vec<f64> = (0..n_freq).map(|i| i as f64 * sr / n_fft as f64).collect();

    let min_mel = hz_to_mel(fmin);
    let max_mel = hz_to_mel(fmax);
    let mel_pts: Vec<f64> = (0..n_mels + 2)
        .map(|i| min_mel + (max_mel - min_mel) * i as f64 / (n_mels + 1) as f64)
        .collect();
    let hz_pts: Vec<f64> = mel_pts.iter().map(|&m| mel_to_hz(m)).collect();
    let fdiff: Vec<f64> = hz_pts.windows(2).map(|w| w[1] - w[0]).collect();

    let mut weights = vec![0f32; n_mels * n_freq];
    for i in 0..n_mels {
        for (k, &f) in fftfreqs.iter().enumerate() {
            let lower = (f - hz_pts[i]) / fdiff[i];
            let upper = (hz_pts[i + 2] - f) / fdiff[i + 1];
            weights[i * n_freq + k] = lower.min(upper).max(0.0) as f32;
        }
        let enorm = 2.0 / (hz_pts[i + 2] - hz_pts[i]);
        for k in 0..n_freq {
            weights[i * n_freq + k] *= enorm as f32;
        }
    }
    weights
}

// --------------------------------------------------------------------------- //
// STFT -> mel -> unit-mel (matches vocoder.py's wav_to_unit_mel to ~1e-6)
// --------------------------------------------------------------------------- //
fn hann_window(n: usize) -> Vec<f64> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()).collect()
}

/// Centered, zero-padded (librosa's current default pad_mode='constant', NOT
/// 'reflect' -- verified empirically) STFT power spectrogram: [n_freq][n_frames].
///
/// Uses a real FFT (rustfft) rather than a naive O(n_fft^2) DFT-matmul. The
/// two are numerically equivalent -- verified against the old DFT code: max
/// relative power difference ~1.2e-9 (f64 accumulation-order noise), which
/// propagates to at most ~9e-11 in the final unit-mel, five orders of
/// magnitude below the ~1.3e-6 tolerance at which the whole pipeline already
/// matches librosa. On a 1s frame set this was ~5000x faster in release
/// (22.4s -> 4ms), and rustfft uses NEON on Apple Silicon, so the win is even
/// larger on the actual target hardware. This is what makes `build-cache`
/// practical at n_fft=4096.
fn stft_power(y: &[f32], n_fft: usize, hop: usize) -> Vec<Vec<f64>> {
    use rustfft::{num_complex::Complex, FftPlanner};

    let pad = n_fft / 2;
    let mut padded = vec![0f32; y.len() + 2 * pad];
    padded[pad..pad + y.len()].copy_from_slice(y);

    let win = hann_window(n_fft);
    let n_freq = n_fft / 2 + 1;
    let n_frames = if padded.len() >= n_fft { 1 + (padded.len() - n_fft) / hop } else { 0 };

    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n_fft);

    let mut power = vec![vec![0f64; n_frames]; n_freq];
    let mut buf = vec![Complex::new(0.0, 0.0); n_fft];
    for t in 0..n_frames {
        let start = t * hop;
        for i in 0..n_fft {
            buf[i] = Complex::new(padded[start + i] as f64 * win[i], 0.0);
        }
        fft.process(&mut buf);
        for k in 0..n_freq {
            power[k][t] = buf[k].re * buf[k].re + buf[k].im * buf[k].im;
        }
    }
    power
}

/// librosa.power_to_db(x, ref=1.0) semantics, including the top_db=80 clamp
/// relative to the GLOBAL max of this call's output (not a fixed floor).
fn power_to_db(mel_power: &[Vec<f64>], ref_val: f64, amin: f64, top_db: f64) -> Vec<Vec<f64>> {
    let mut log_spec: Vec<Vec<f64>> = mel_power
        .iter()
        .map(|row| row.iter().map(|&p| 10.0 * p.max(amin).log10() - 10.0 * ref_val.max(amin).log10()).collect())
        .collect();
    let max_val = log_spec.iter().flatten().cloned().fold(f64::MIN, f64::max);
    let floor = max_val - top_db;
    for row in log_spec.iter_mut() {
        for v in row.iter_mut() {
            if *v < floor {
                *v = floor;
            }
        }
    }
    log_spec
}

/// Full pipeline: waveform -> unit-mel in [-1,1], row-major [n_mels, n_frames].
/// Matches gan-zai-v5.2.py/vocoder.py's `wav_to_unit_mel` / `db_to_unit` to ~1e-6.
pub fn wav_to_unit_mel(y: &[f32], sr: f64, n_fft: usize, hop: usize, n_mels: usize, db_floor: f64, db_ceil: f64) -> (Vec<f32>, usize) {
    let power = stft_power(y, n_fft, hop);
    let n_freq = power.len();
    let n_frames = if n_freq > 0 { power[0].len() } else { 0 };
    let melfb = mel_filterbank(sr, n_fft, n_mels);

    let mut mel_power = vec![vec![0f64; n_frames]; n_mels];
    for m in 0..n_mels {
        for t in 0..n_frames {
            let mut s = 0f64;
            for k in 0..n_freq {
                s += melfb[m * n_freq + k] as f64 * power[k][t];
            }
            mel_power[m][t] = s;
        }
    }
    let db = power_to_db(&mel_power, 1.0, 1e-10, 80.0);
    let mut out = vec![0f32; n_mels * n_frames];
    for m in 0..n_mels {
        for t in 0..n_frames {
            let d = db[m][t].clamp(db_floor, db_ceil);
            out[m * n_frames + t] = ((d - db_floor) / (db_ceil - db_floor) * 2.0 - 1.0) as f32;
        }
    }
    (out, n_frames)
}
