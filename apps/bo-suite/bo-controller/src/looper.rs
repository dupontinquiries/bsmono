// src/looper.rs
#![allow(dead_code)]
//
// Soundscape loop finder.
//
// Given a stereo audio buffer, find the (start, end) sample pair that
// produces the cleanest possible perceptual loop. Two failure modes
// to design against:
//
//   1. AUDIO POPPING — when the sample value at `end` differs sharply
//      from the value at `start`, the loop seam creates a discontinuity
//      that the ear hears as a click or pop. Worse: a discontinuity in
//      the first DERIVATIVE (slope) also pops, even when the values
//      match exactly. Both must be minimized.
//
//   2. MFCC TEMPORAL VARIATION — when the spectral / timbral character
//      at the start differs from the end, the loop has a "swelling" or
//      "settling" feel that the ear hears even when there's no actual
//      click. Quantified via Mel-Frequency Cepstral Coefficient
//      distance between a short window at the start and a short window
//      at the end.
//
// Search strategy: random-walk many candidate (start, end) pairs in
// the legal range, score each one, then refine the top K with local
// perturbations. Pure greedy local search gets stuck in noise; pure
// random walk wastes work on bad regions. The hybrid is robust.
//
// MFCC pipeline (no external FFT library — hand-rolled radix-2 to
// keep the dependency surface tiny):
//   * Window (Hamming) → FFT → power spectrum → mel filterbank →
//     log10 → DCT → first 13 coefficients (skip the 0th, which is
//     just energy).
//
// All scores are normalized so the weights in LoopParams are
// commensurate (each component is roughly in 0..1 range for typical
// audio).

use std::f32::consts::PI;
use std::path::Path;

/// User-tunable parameters for the loop search.
#[derive(Debug, Clone)]
pub struct LoopParams {
    /// Desired loop length in seconds. The search prefers (but does
    /// not strictly require) candidates near this length.
    pub target_seconds: f32,
    /// Tolerance band around target_seconds — outside this band the
    /// length penalty grows quadratically.
    pub tolerance_seconds: f32,
    /// Number of random candidates to sample in the broad-search phase.
    pub n_candidates: usize,
    /// Number of best candidates to refine with local perturbation.
    pub n_refine: usize,
    /// Number of perturbation steps per refinement candidate.
    pub n_refine_steps: usize,
    /// Weight on sample-value discontinuity at the seam.
    pub w_pop: f32,
    /// Weight on MFCC distance between boundary windows.
    pub w_mfcc: f32,
    /// Weight on length penalty (deviation from target_seconds).
    pub w_length: f32,
    /// PRNG seed for reproducible searches. Set differently per
    /// run if randomness is desired.
    pub seed: u64,
}

impl Default for LoopParams {
    fn default() -> Self {
        Self {
            target_seconds: 8.0,
            tolerance_seconds: 4.0,
            n_candidates: 2000,
            n_refine: 16,
            n_refine_steps: 200,
            w_pop: 1.0,
            w_mfcc: 1.0,
            w_length: 0.3,
            seed: 0xB0C0_DEED_C0FE_F00D,
        }
    }
}

/// Final loop selection.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LoopResult {
    pub start: usize,
    pub end: usize,
    pub score: f32,
    pub pop_score: f32,
    pub mfcc_score: f32,
    pub length_score: f32,
    pub length_seconds: f32,
}

/// Mono float audio buffer. We collapse stereo to mono for MFCC
/// computation (timbral comparison) but keep the original stereo
/// data for sample-level pop scoring + final output.
pub struct SoundBuffer {
    pub samples_lr: Vec<(f32, f32)>,    // interleaved as pairs for fast indexing
    pub sample_rate: u32,
}

impl SoundBuffer {
    pub fn n_samples(&self) -> usize { self.samples_lr.len() }
    pub fn duration_seconds(&self) -> f32 {
        self.samples_lr.len() as f32 / self.sample_rate as f32
    }

    /// Mono sample at index `i`, average of L and R. Returns 0.0
    /// when out of bounds (callers should bounds-check first).
    fn mono(&self, i: usize) -> f32 {
        if i >= self.samples_lr.len() { return 0.0; }
        let (l, r) = self.samples_lr[i];
        (l + r) * 0.5
    }

    /// Copy `n` mono samples starting at `start` into `out`.
    /// Zero-pads if the range extends past the buffer.
    fn mono_window(&self, start: usize, n: usize, out: &mut [f32]) {
        let end = (start + n).min(self.samples_lr.len());
        for (i, sample) in out.iter_mut().enumerate() {
            let src = start + i;
            *sample = if src < end {
                let (l, r) = self.samples_lr[src];
                (l + r) * 0.5
            } else { 0.0 };
        }
    }
}

// ── WAV I/O ─────────────────────────────────────────────────────────

/// Decode a 16-bit PCM WAV file to a float SoundBuffer. We accept
/// any channel count and downmix to stereo if needed; mono inputs
/// duplicate to both channels.
pub fn read_wav(path: &Path) -> Result<SoundBuffer, String> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| format!("open {}: {}", path.display(), e))?;
    let spec = reader.spec();
    if !matches!(spec.sample_format, hound::SampleFormat::Int) {
        return Err(format!("only PCM int WAV supported (got {:?})", spec.sample_format));
    }
    let max_amp = match spec.bits_per_sample {
        16 => 32768.0,
        24 => 8_388_608.0,
        32 => 2_147_483_648.0,
        bps => return Err(format!("unsupported bit depth: {}", bps)),
    };
    let samples_i32: Vec<i32> = reader.samples::<i32>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("decode: {}", e))?;

    let chans = spec.channels.max(1) as usize;
    let n_frames = samples_i32.len() / chans;
    let mut samples_lr = Vec::with_capacity(n_frames);
    for f in 0..n_frames {
        let base = f * chans;
        let (l, r) = match chans {
            1 => {
                let m = samples_i32[base] as f32 / max_amp;
                (m, m)
            }
            2 => (
                samples_i32[base] as f32 / max_amp,
                samples_i32[base + 1] as f32 / max_amp,
            ),
            _ => {
                // Downmix multichannel to stereo: average first half
                // to L, second half to R. Crude but defensible.
                let mid = chans / 2;
                let mut l = 0.0_f32;
                let mut r = 0.0_f32;
                for c in 0..mid {
                    l += samples_i32[base + c] as f32 / max_amp;
                }
                for c in mid..chans {
                    r += samples_i32[base + c] as f32 / max_amp;
                }
                (l / mid as f32, r / (chans - mid) as f32)
            }
        };
        samples_lr.push((l, r));
    }
    Ok(SoundBuffer { samples_lr, sample_rate: spec.sample_rate })
}

/// Decode any supported audio file to a float SoundBuffer. WAV is read
/// directly (fast, no subprocess); everything else — Opus, mp3, flac,
/// m4a/aac, ogg vorbis — is decoded to a temporary 16-bit PCM WAV via
/// ffmpeg (already a hard dependency for playback) and then read with the
/// existing WAV path. This lets the soundscape workshop accept compressed
/// sources (notably .opus, which is what it *outputs*, so a rendered loop
/// can be re-fed as input) rather than WAV only.
///
/// The temp file is created in the system temp dir and removed before
/// returning, regardless of success, so we never leak large PCM scratch
/// files. ffmpeg is invoked with explicit stereo s16 output so the WAV
/// reader's fast path always applies.
pub fn read_audio(path: &Path) -> Result<SoundBuffer, String> {
    let is_wav = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("wav"))
        .unwrap_or(false);
    if is_wav {
        return read_wav(path);
    }
    if !path.exists() {
        return Err(format!("input not found: {}", path.display()));
    }

    // Decode to a temp WAV via ffmpeg. Unique-ish name from pid + nanos to
    // avoid collisions between concurrent renders.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!(
        "bo_looper_decode_{}_{}.wav", std::process::id(), nanos));

    let status = std::process::Command::new("ffmpeg")
        .args([
            "-v", "error",
            "-y",                       // overwrite temp if it somehow exists
            "-i", &path.to_string_lossy(),
            "-ac", "2",                 // force stereo
            "-c:a", "pcm_s16le",        // 16-bit PCM → read_wav fast path
            "-f", "wav",
        ])
        .arg(&tmp)
        .status();

    let decoded = match status {
        Ok(s) if s.success() => read_wav(&tmp),
        Ok(s) => Err(format!(
            "ffmpeg failed to decode {} (exit {:?}); is the file a valid \
             audio file?", path.display(), s.code())),
        Err(e) => Err(format!(
            "could not run ffmpeg to decode {}: {} (ffmpeg is required for \
             non-WAV input)", path.display(), e)),
    };

    // Always clean up the scratch file.
    let _ = std::fs::remove_file(&tmp);
    decoded
}

/// Write the looped audio as an OggOpus file. Opus is the right
/// format for soundscapes: typical bitrates of 96–128 kbps give
/// perceptually-transparent quality on ambient loops, and the
/// resulting file is roughly 14× smaller than the equivalent WAV.
/// Mozart firmware on B&O speakers plays OggOpus natively via the
/// DLNA renderer, so we can hand the file straight to a soundscape
/// player without further transcoding.
///
/// The Opus codec is fixed at 48 kHz internally — we resample on the
/// fly (linear interpolation; soundscapes are usually low-bandwidth
/// nature/ambient sources so the resampler quality is sufficient).
///
/// `fade_samples` applies a crossfade across the loop seam in the
/// SOURCE sample rate; the crossfaded material is then resampled and
/// encoded into the Opus stream.
///
/// Container format: OggOpus per RFC 7845. We emit one Ogg stream
/// containing the OpusHead identification header, an OpusTags
/// comment header (intentionally minimal — no application-specific
/// metadata), and the encoded audio packets at 20 ms frame size.
pub fn write_loop_opus(buf: &SoundBuffer, result: &LoopResult,
                      fade_samples: usize, bitrate_kbps: u32,
                      output: &Path) -> Result<(), String>
{
    if result.end <= result.start { return Err("end <= start".into()); }
    let loop_len = result.end - result.start;
    let fade = fade_samples.min(loop_len / 4);
    let src_sr = buf.sample_rate;

    // ── Step 1: build a contiguous float buffer holding ONE pass of
    // the loop, with crossfade applied at the seam (last `fade`
    // samples linearly mixed with the first `fade` of the loop).
    // We keep stereo interleaved so the resampler treats both
    // channels in lockstep.
    let mut src_pcm: Vec<(f32, f32)> = Vec::with_capacity(loop_len);
    for i in 0..loop_len {
        let (mut l, mut r) = buf.samples_lr[result.start + i];
        if fade > 0 && i >= loop_len - fade {
            let t = (i - (loop_len - fade)) as f32 / fade as f32;
            let (sl, sr) = buf.samples_lr[result.start + (i - (loop_len - fade))];
            l = l * (1.0 - t) + sl * t;
            r = r * (1.0 - t) + sr * t;
        }
        src_pcm.push((l, r));
    }

    // ── Step 2: resample to 48 kHz. Linear interpolation is fine
    // for soundscape material — the spectral top end is usually
    // attenuated by the ambient nature of the source, and the Opus
    // encoder reconstructs at high quality.
    let pcm_48k = linear_resample_to_48k(&src_pcm, src_sr);

    // ── Step 3: encode + write.
    encode_pcm_to_opus(&pcm_48k, bitrate_kbps, output)
}

/// Linear-interpolation resample a stereo float buffer to 48 kHz
/// (the Opus encoder's native rate). When the source is already at
/// 48 kHz the input is returned unchanged via an owned clone.
///
/// Sufficient for the use cases we have (soundscapes — wide-band
/// ambient material; mixed audio — perceptually compressed Opus
/// output). A higher-order resampler (e.g. sinc) would be cleaner
/// for high-fidelity work but adds complexity we don't yet need.
pub fn linear_resample_to_48k(src: &[(f32, f32)], src_sr: u32) -> Vec<(f32, f32)> {
    let dst_sr = 48_000_u32;
    if src_sr == dst_sr || src.is_empty() {
        return src.to_vec();
    }
    // src_sr == 0 from a malformed WAV header would otherwise produce
    // ratio = 0.0 → dst_len = src_len / 0 = Inf → `Inf as usize` saturates
    // to usize::MAX → `Vec::with_capacity(usize::MAX)` panics. Treat as
    // "I have no idea what rate this is" and pass through unchanged;
    // the caller can decide whether to reject it at a higher level.
    if src_sr == 0 {
        return src.to_vec();
    }
    let ratio = src_sr as f64 / dst_sr as f64;
    let src_len = src.len();
    let dst_len = ((src_len as f64) / ratio).round() as usize;
    let mut out = Vec::with_capacity(dst_len);
    for i in 0..dst_len {
        let src_pos = (i as f64) * ratio;
        let i0 = (src_pos.floor() as usize).min(src_len - 1);
        let i1 = (i0 + 1).min(src_len - 1);
        let frac = (src_pos - i0 as f64) as f32;
        let (l0, r0) = src[i0];
        let (l1, r1) = src[i1];
        out.push((
            l0 + (l1 - l0) * frac,
            r0 + (r1 - r0) * frac,
        ));
    }
    out
}

/// Encode a stereo PCM buffer (already at 48 kHz) to an OggOpus file
/// per RFC 7845. Shared by the soundscape loop encoder and the
/// stream-mixer file processor — both pipelines need the same Opus
/// container format with identical headers, so consolidating here
/// guarantees byte-level consistency.
///
/// `samples_48k` must be stereo at 48 kHz (use linear_resample_to_48k
/// if the source is at a different rate). `bitrate_kbps` is the Opus
/// bitrate target (96 kbps is a good default for soundscapes and
/// 128 kbps for music).
pub fn encode_pcm_to_opus(samples_48k: &[(f32, f32)],
                          bitrate_kbps: u32,
                          output: &Path) -> Result<(), String>
{
    use std::fs::File;
    use std::io::Write;

    // ── Configure the Opus encoder. Frame size is 20 ms, which is
    // 960 samples at 48 kHz — the most common choice and a sweet
    // spot for quality vs. latency.
    const FRAME_SAMPLES: usize = 960;   // 48000 * 0.020
    let dst_sr = 48_000_u32;
    let mut encoder = opus::Encoder::new(
        dst_sr,
        opus::Channels::Stereo,
        opus::Application::Audio,
    ).map_err(|e| format!("opus encoder init: {}", e))?;
    encoder.set_bitrate(opus::Bitrate::Bits((bitrate_kbps * 1000) as i32))
           .map_err(|e| format!("opus set_bitrate: {}", e))?;

    // ── Open the output and emit the OggOpus stream.
    let file = File::create(output)
        .map_err(|e| format!("create {}: {}", output.display(), e))?;
    let mut writer = ogg::writing::PacketWriter::new(file);
    // Serial number — for a single self-contained stream any
    // value works; we derive from the sample count + a constant
    // so repeated encodes of the same buffer are byte-identical.
    let serial = (samples_48k.len() as u32).wrapping_mul(2654435761) ^ 0xB0CDEEF;

    // OpusHead per RFC 7845 §5.1. 19 bytes.
    let mut opus_head = Vec::with_capacity(19);
    opus_head.extend_from_slice(b"OpusHead");
    opus_head.push(1);                                     // version
    opus_head.push(2);                                     // channels
    opus_head.extend_from_slice(&3840u16.to_le_bytes());   // pre-skip
    opus_head.extend_from_slice(&dst_sr.to_le_bytes());    // input sr
    opus_head.extend_from_slice(&0i16.to_le_bytes());      // gain
    opus_head.push(0);                                     // mapping family
    writer.write_packet(opus_head, serial,
        ogg::writing::PacketWriteEndInfo::EndPage, 0)
        .map_err(|e| format!("ogg write head: {}", e))?;

    // OpusTags per §5.2.
    let mut opus_tags = Vec::new();
    opus_tags.extend_from_slice(b"OpusTags");
    let vendor = b"bo-controller/encoder";
    opus_tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    opus_tags.extend_from_slice(vendor);
    opus_tags.extend_from_slice(&0u32.to_le_bytes());   // 0 comments
    writer.write_packet(opus_tags, serial,
        ogg::writing::PacketWriteEndInfo::EndPage, 0)
        .map_err(|e| format!("ogg write tags: {}", e))?;

    // ── Stream the encoded audio packets.
    let n_frames = samples_48k.len();
    let mut pos: usize = 0;
    let mut packet_buf = vec![0u8; 4096];
    let mut granule: u64 = 3840;
    while pos < n_frames {
        let remaining = n_frames - pos;
        let take = remaining.min(FRAME_SAMPLES);
        let mut interleaved = [0.0_f32; FRAME_SAMPLES * 2];
        for i in 0..take {
            let (l, r) = samples_48k[pos + i];
            interleaved[i * 2]     = l;
            interleaved[i * 2 + 1] = r;
        }
        let n_bytes = encoder.encode_float(&interleaved, &mut packet_buf)
            .map_err(|e| format!("opus encode at frame {}: {}", pos, e))?;
        pos += FRAME_SAMPLES;
        granule += FRAME_SAMPLES as u64;
        let is_last = pos >= n_frames;
        let end_info = if is_last {
            ogg::writing::PacketWriteEndInfo::EndStream
        } else {
            ogg::writing::PacketWriteEndInfo::NormalPacket
        };
        writer.write_packet(packet_buf[..n_bytes].to_vec(),
                            serial, end_info, granule)
            .map_err(|e| format!("ogg write packet at frame {}: {}", pos, e))?;
    }
    writer.into_inner().flush()
        .map_err(|e| format!("flush: {}", e))?;
    Ok(())
}

// Marker so the rest of the original write_loop_opus function gets
// removed cleanly below.
#[allow(dead_code)] fn _looper_refactor_anchor() {}

// ── FFT (hand-rolled radix-2 Cooley-Tukey) ──────────────────────────
//
// Operates in-place on a Vec<(f32, f32)> as (real, imag). Input length
// must be a power of two; callers either pick a power-of-two window
// or zero-pad to one.

pub fn fft(buf: &mut [(f32, f32)]) {
    let n = buf.len();
    if n == 0 || (n & (n - 1)) != 0 {
        panic!("fft requires power-of-two length, got {}", n);
    }
    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j { buf.swap(i, j); }
    }
    // Butterfly.
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let ang = -2.0 * PI / len as f32;
        let wlen = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let mut w = (1.0_f32, 0.0_f32);
            for k in 0..half {
                let (xr, xi) = buf[i + k];
                let (yr, yi) = buf[i + k + half];
                // (yr + j*yi) * (w.0 + j*w.1)
                let tr = yr * w.0 - yi * w.1;
                let ti = yr * w.1 + yi * w.0;
                buf[i + k]        = (xr + tr, xi + ti);
                buf[i + k + half] = (xr - tr, xi - ti);
                let wn = (w.0 * wlen.0 - w.1 * wlen.1,
                          w.0 * wlen.1 + w.1 * wlen.0);
                w = wn;
            }
            i += len;
        }
        len <<= 1;
    }
}

// ── MFCC pipeline ───────────────────────────────────────────────────

/// Number of mel filterbank bins.
const MEL_N: usize = 26;
/// Number of MFCC coefficients to keep (skip coefficient 0 which is
/// just total log-energy and doesn't help timbral comparison).
const MFCC_N: usize = 13;
/// FFT window length. 1024 samples ≈ 23 ms at 44.1 kHz — small enough
/// to be sensitive to the loop seam, large enough for decent spectral
/// resolution down to a few hundred Hz.
const FFT_N: usize = 1024;

/// Compute an MFCC vector for `signal` (length must be FFT_N or padded).
/// Returns MFCC_N coefficients.
pub fn compute_mfcc(signal: &[f32], sample_rate: u32) -> [f32; MFCC_N] {
    // 1. Window with Hamming.
    let mut buf: Vec<(f32, f32)> = (0..FFT_N).map(|i| {
        let s = if i < signal.len() { signal[i] } else { 0.0 };
        let w = 0.54 - 0.46 * (2.0 * PI * i as f32 / (FFT_N - 1) as f32).cos();
        (s * w, 0.0)
    }).collect();

    // 2. FFT.
    fft(&mut buf);

    // 3. Power spectrum (only positive frequencies, FFT_N/2 + 1 bins).
    let n_bins = FFT_N / 2 + 1;
    let power: Vec<f32> = buf[..n_bins].iter()
        .map(|(re, im)| re * re + im * im)
        .collect();

    // 4. Mel filterbank.
    let mel_energies = mel_filterbank(&power, sample_rate, MEL_N);

    // 5. log10 (with floor to avoid -inf on silent bins).
    let log_mel: Vec<f32> = mel_energies.iter()
        .map(|e| (e.max(1e-10)).log10())
        .collect();

    // 6. DCT type-II.
    dct_first_n(&log_mel)
}

/// Convert Hz → mel scale (Slaney convention).
fn hz_to_mel(hz: f32) -> f32 { 2595.0 * (1.0 + hz / 700.0).log10() }
fn mel_to_hz(mel: f32) -> f32 { 700.0 * (10.0_f32.powf(mel / 2595.0) - 1.0) }

/// Compute mel filterbank energies from a power spectrum.
pub fn mel_filterbank(power: &[f32], sample_rate: u32, n_filters: usize) -> Vec<f32> {
    let nyquist = sample_rate as f32 / 2.0;
    let n_bins = power.len();
    // n_filters + 2 mel-spaced points define n_filters triangular filters.
    let mel_low = hz_to_mel(0.0);
    let mel_high = hz_to_mel(nyquist);
    let points: Vec<f32> = (0..=(n_filters + 1)).map(|i| {
        let mel = mel_low + (mel_high - mel_low) * i as f32 / (n_filters + 1) as f32;
        let hz = mel_to_hz(mel);
        // Bin index for this Hz.
        hz / nyquist * (n_bins - 1) as f32
    }).collect();

    (0..n_filters).map(|m| {
        let (lo, mid, hi) = (points[m], points[m + 1], points[m + 2]);
        let mut sum = 0.0_f32;
        let bin_start = lo.floor() as usize;
        let bin_end = hi.ceil() as usize + 1;
        for b in bin_start..bin_end.min(n_bins) {
            let bf = b as f32;
            let w = if bf < lo || bf > hi { 0.0 }
                    else if bf <= mid {
                        if mid - lo > 0.0 { (bf - lo) / (mid - lo) } else { 0.0 }
                    } else {
                        if hi - mid > 0.0 { (hi - bf) / (hi - mid) } else { 0.0 }
                    };
            sum += w * power[b];
        }
        sum
    }).collect()
}

/// DCT-II keeping the first MFCC_N coefficients (skipping coeff 0).
fn dct_first_n(input: &[f32]) -> [f32; MFCC_N] {
    let n = input.len();
    let mut out = [0.0_f32; MFCC_N];
    for k in 0..MFCC_N {
        // We're computing coefficients 1..=MFCC_N (skip 0).
        let k_actual = k + 1;
        let mut sum = 0.0_f32;
        for (i, &x) in input.iter().enumerate() {
            sum += x * (PI / n as f32 * (i as f32 + 0.5) * k_actual as f32).cos();
        }
        out[k] = sum;
    }
    out
}

/// Average power spectrum of a WAV file, expressed as log-spaced
/// (freq_hz, db) points across `lo_hz..hi_hz`. This is the third
/// layer of the mixer's "groups make sense" verification: the user
/// can overlay their source material's actual spectral content on
/// the same axes as the EQ response curves, then SEE whether their
/// settings target real peaks or are guessing.
///
/// Algorithm:
///   • Window the file into FFT_N-sample frames (no overlap).
///   • Hamming window each frame and FFT it.
///   • Average the resulting power spectra to smooth transient
///     energy peaks into a stable average.
///   • Linearly interpolate the linear-frequency bins onto a
///     log-frequency output grid that matches RealtimeMixer's
///     compute_curves() — so the canvas can draw them aligned.
///
/// Up to `max_frames` frames are taken; for long files we stride
/// across the file rather than scan every frame, since the average
/// stabilizes after a few seconds of material.
pub fn spectrum_average(
    path: &Path,
    n_points: usize,
    lo_hz: f32,
    hi_hz: f32,
    max_frames: usize,
) -> Result<Vec<(f32, f32)>, String>
{
    let buf = read_wav(path)?;
    if buf.n_samples() < FFT_N {
        return Err(format!("file too short — need at least {} samples", FFT_N));
    }

    // Distribute up to `max_frames` non-overlapping windows across
    // the file. For a 30-second WAV at 44.1 kHz this is ≈ 1300
    // potential windows; we want maybe 200 to keep the FFT cost
    // reasonable while averaging out transient variation.
    let total_windows = buf.n_samples() / FFT_N;
    if total_windows == 0 {
        return Err("file too short for any FFT window".into());
    }
    let n_take = total_windows.min(max_frames).max(1);
    let stride = (total_windows / n_take).max(1);

    let n_bins = FFT_N / 2 + 1;
    let mut power_sum = vec![0.0_f64; n_bins];
    let mut sample_buf = vec![0.0_f32; FFT_N];
    let mut windows_used = 0_usize;

    for w in 0..n_take {
        let start = w * stride * FFT_N;
        if start + FFT_N > buf.n_samples() { break; }
        buf.mono_window(start, FFT_N, &mut sample_buf);

        // Hamming window — small mainlobe + 43 dB sidelobe rejection,
        // standard pick for general-purpose spectral averaging.
        let mut fft_buf: Vec<(f32, f32)> = (0..FFT_N).map(|i| {
            let w_coef = 0.54 - 0.46 * (2.0 * PI * i as f32 / (FFT_N - 1) as f32).cos();
            (sample_buf[i] * w_coef, 0.0)
        }).collect();
        fft(&mut fft_buf);
        for (i, slot) in power_sum.iter_mut().enumerate() {
            let (re, im) = fft_buf[i];
            *slot += (re * re + im * im) as f64;
        }
        windows_used += 1;
    }

    if windows_used == 0 {
        return Err("no usable FFT windows".into());
    }
    let scale = 1.0 / windows_used as f64;
    for p in power_sum.iter_mut() { *p *= scale; }

    // Map to log-spaced output bins. The reference level is set so
    // that the loudest content tends to land near 0 dB and the noise
    // floor sits well below — chosen empirically so the spectrum
    // overlays comfortably on the ±18 dB axis the mixer canvas uses.
    let nyquist = buf.sample_rate as f32 / 2.0;
    let log_lo = lo_hz.max(1.0).ln();
    let log_hi = hi_hz.max(lo_hz + 1.0).ln();

    let mut out = Vec::with_capacity(n_points);
    for i in 0..n_points {
        let t = i as f32 / (n_points.max(2) - 1) as f32;
        let freq = (log_lo + (log_hi - log_lo) * t).exp();
        let bin_pos = freq / nyquist * (n_bins - 1) as f32;
        let i0 = (bin_pos.floor() as usize).min(n_bins - 1);
        let i1 = (i0 + 1).min(n_bins - 1);
        let frac = (bin_pos - i0 as f32) as f64;
        let p = power_sum[i0] * (1.0 - frac) + power_sum[i1] * frac;
        // 10·log₁₀(power), shifted down so typical peaks land at 0 dB.
        let db = 10.0 * (p.max(1e-20).log10() as f32) - 60.0;
        out.push((freq, db));
    }
    Ok(out)
}

// ── Scoring ─────────────────────────────────────────────────────────

/// Score a candidate (start, end) loop. Lower is better.
/// Returns (total_score, pop_score, mfcc_score, length_score) so the
/// caller can record the breakdown.
pub fn score_candidate(
    buf: &SoundBuffer,
    start: usize,
    end: usize,
    params: &LoopParams,
    mfcc_at: &dyn Fn(usize) -> [f32; MFCC_N],
) -> (f32, f32, f32, f32) {
    if end <= start || end >= buf.n_samples() {
        return (f32::INFINITY, 0.0, 0.0, 0.0);
    }
    // POP: sample discontinuity at the seam, both value AND derivative.
    // We average L and R channels separately to avoid stereo-specific
    // imbalance dominating the score.
    let (sl, sr) = buf.samples_lr[start];
    let (el, er) = buf.samples_lr[end];
    // Derivative comparison: slope just before end vs slope just after start.
    let (sl1, sr1) = buf.samples_lr.get(start + 1).copied().unwrap_or((0.0, 0.0));
    let (el1, er1) = if end >= 1 { buf.samples_lr[end - 1] } else { (0.0, 0.0) };
    let slope_start_l = sl1 - sl;
    let slope_start_r = sr1 - sr;
    let slope_end_l = el - el1;
    let slope_end_r = er - er1;
    let value_disc = ((el - sl).abs() + (er - sr).abs()) * 0.5;
    let slope_disc = ((slope_end_l - slope_start_l).abs()
                    + (slope_end_r - slope_start_r).abs()) * 0.5;
    // Combine with a slight tilt toward value: a click is more audible
    // than a glitch. Normalize: typical max for 16-bit normalized
    // audio is 2.0 (waveform range -1..1, max disc 2.0).
    let pop = (value_disc * 0.7 + slope_disc * 0.3) / 2.0;

    // MFCC: timbral distance between start window and end window.
    let mfcc_start = mfcc_at(start);
    let mfcc_end = mfcc_at(end.saturating_sub(FFT_N));
    let mut mfcc_dist = 0.0_f32;
    for i in 0..MFCC_N {
        let d = mfcc_start[i] - mfcc_end[i];
        mfcc_dist += d * d;
    }
    mfcc_dist = mfcc_dist.sqrt();
    // Empirical normalization — MFCC L2 typical range is 0..100.
    let mfcc_norm = (mfcc_dist / 50.0).min(1.0);

    // Length penalty.
    let length_s = (end - start) as f32 / buf.sample_rate as f32;
    let dev = (length_s - params.target_seconds).abs();
    let normalized_dev = (dev - params.tolerance_seconds).max(0.0)
                        / params.tolerance_seconds.max(0.001);
    let length_score = normalized_dev * normalized_dev;

    let total = params.w_pop * pop
              + params.w_mfcc * mfcc_norm
              + params.w_length * length_score;
    (total, pop, mfcc_norm, length_score)
}

// ── Random-walk search ──────────────────────────────────────────────

/// Tiny seeded PRNG (xorshift64). Avoids pulling rand as a dependency.
struct Xorshift64(u64);
impl Xorshift64 {
    fn new(seed: u64) -> Self { Self(seed.max(1)) }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        self.0 = x;
        x
    }
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        if hi <= lo { return lo; }
        lo + (self.next_u64() as usize) % (hi - lo)
    }
    fn jitter(&mut self, n: usize, max: usize) -> i64 {
        let r = (self.next_u64() % (2 * max as u64 + 1)) as i64 - max as i64;
        (n as i64 + r).max(0) as i64 - n as i64
    }
}

/// Find the best loop point via random walk + local refinement.
/// `progress` is called with values in [0.0, 1.0] at regular intervals
/// so the QML side can drive a progress bar.
pub fn find_best_loop(
    buf: &SoundBuffer,
    params: &LoopParams,
    mut progress: impl FnMut(f32),
) -> LoopResult {
    let sr = buf.sample_rate as usize;
    let target_len = (params.target_seconds * sr as f32) as usize;
    let tol_len    = (params.tolerance_seconds * sr as f32) as usize;
    let min_len = target_len.saturating_sub(tol_len * 2).max(sr / 4);   // ≥ 0.25 s
    let max_len = target_len + tol_len * 2;
    let n = buf.n_samples();
    if n < min_len + FFT_N * 2 {
        // Too short to loop usefully — return a trivial loop covering
        // the whole buffer (the caller will see the length_score is
        // huge and treat it as a failure case).
        return LoopResult {
            start: 0, end: n.saturating_sub(1),
            score: f32::INFINITY,
            pop_score: 0.0, mfcc_score: 0.0, length_score: f32::INFINITY,
            length_seconds: n as f32 / sr as f32,
        };
    }

    // Pre-compute MFCC at every `hop` samples. This converts a per-
    // candidate MFCC computation (slow) into a lookup (fast).
    let hop = (sr / 50).max(256);   // ≈ 20 ms hop
    let n_frames = (n.saturating_sub(FFT_N)) / hop + 1;
    let mut mfcc_grid: Vec<[f32; MFCC_N]> = Vec::with_capacity(n_frames);
    let mut window_buf = vec![0.0_f32; FFT_N];
    for f in 0..n_frames {
        let start = f * hop;
        buf.mono_window(start, FFT_N, &mut window_buf);
        mfcc_grid.push(compute_mfcc(&window_buf, buf.sample_rate));
    }
    let mfcc_at = |sample_index: usize| -> [f32; MFCC_N] {
        let frame = (sample_index / hop).min(mfcc_grid.len() - 1);
        mfcc_grid[frame]
    };

    let mut rng = Xorshift64::new(params.seed);
    // Reserve room for at least one sample-of-tail past `end` for the
    // derivative comparison; min_start = FFT_N/2 to give the start MFCC
    // some headroom.
    let lo = FFT_N;
    let hi = n.saturating_sub(min_len + FFT_N);

    // Broad random search.
    let mut best: Option<LoopResult> = None;
    for i in 0..params.n_candidates {
        let start = rng.range(lo, hi.max(lo + 1));
        let target_end = start + rng.range(min_len, max_len.min(n - FFT_N - start));
        let end = target_end.min(n - 1);
        let (score, pop, mfcc, ls) = score_candidate(buf, start, end, params, &mfcc_at);
        if best.as_ref().map(|b| score < b.score).unwrap_or(true) {
            best = Some(LoopResult {
                start, end, score, pop_score: pop, mfcc_score: mfcc,
                length_score: ls,
                length_seconds: (end - start) as f32 / sr as f32,
            });
        }
        if i % 64 == 0 {
            progress(0.5 * i as f32 / params.n_candidates as f32);
        }
    }

    // Local refinement around the best candidate. Pure local-search
    // gets stuck in noise, but starting from the best random hit and
    // taking small jittered steps reliably improves it.
    if let Some(ref mut current) = best {
        let max_jitter = (sr / 100).max(64);  // ±10 ms or so
        for i in 0..params.n_refine_steps {
            let ds = rng.jitter(current.start, max_jitter);
            let de = rng.jitter(current.end, max_jitter);
            let new_start = (current.start as i64 + ds).max(lo as i64) as usize;
            let new_end   = (current.end   as i64 + de).max(0).min((n - 1) as i64) as usize;
            if new_end <= new_start + min_len / 4 { continue; }
            let (score, pop, mfcc, ls) =
                score_candidate(buf, new_start, new_end, params, &mfcc_at);
            if score < current.score {
                *current = LoopResult {
                    start: new_start, end: new_end, score,
                    pop_score: pop, mfcc_score: mfcc, length_score: ls,
                    length_seconds: (new_end - new_start) as f32 / sr as f32,
                };
            }
            if i % 16 == 0 {
                progress(0.5 + 0.5 * i as f32 / params.n_refine_steps as f32);
            }
        }
    }
    progress(1.0);
    best.unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, freq: f32, sr: u32) -> SoundBuffer {
        let dt = 1.0 / sr as f32;
        let samples_lr = (0..n).map(|i| {
            let s = (2.0 * PI * freq * i as f32 * dt).sin();
            (s, s)
        }).collect();
        SoundBuffer { samples_lr, sample_rate: sr }
    }

    #[test]
    fn fft_roundtrip_on_dc() {
        let mut buf = vec![(1.0, 0.0); 16];
        fft(&mut buf);
        // For DC input, all energy ends up in bin 0; others ≈ 0.
        assert!((buf[0].0 - 16.0).abs() < 1e-3);
        for (re, im) in &buf[1..] {
            assert!(re.abs() < 1e-3 && im.abs() < 1e-3,
                "bin had energy: ({}, {})", re, im);
        }
    }

    #[test]
    fn fft_localizes_sine() {
        let n = 64;
        let bin = 4;        // 4 full cycles → energy in bin 4
        let mut buf: Vec<(f32, f32)> = (0..n).map(|i| {
            ((2.0 * PI * bin as f32 * i as f32 / n as f32).sin(), 0.0)
        }).collect();
        fft(&mut buf);
        let power: Vec<f32> = buf.iter().map(|(r, i)| r*r + i*i).collect();
        let max_bin = power.iter().enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        // Real-valued sine produces conjugate pairs; energy is at bin
        // 4 and bin n-4. Either is acceptable.
        assert!(max_bin == bin || max_bin == n - bin,
            "expected bin {} or {}, got {}", bin, n - bin, max_bin);
    }

    #[test]
    #[should_panic]
    fn fft_rejects_non_power_of_two() {
        let mut buf = vec![(0.0, 0.0); 13];
        fft(&mut buf);
    }

    #[test]
    fn looper__resample_with_zero_src_rate_does_not_panic() {
        // A malformed WAV header could yield src_sr = 0; without
        // defense the resample would divide by zero, compute
        // dst_len = Inf → usize::MAX, and Vec::with_capacity would
        // panic with capacity overflow. Defense returns the source
        // unchanged.
        let src = vec![(0.1_f32, 0.2_f32); 100];
        let out = linear_resample_to_48k(&src, 0);
        assert_eq!(out.len(), 100, "should pass through, not panic");
    }

    #[test]
    fn hz_to_mel_monotonic() {
        let m0 = hz_to_mel(0.0);
        let m1 = hz_to_mel(1000.0);
        let m2 = hz_to_mel(10000.0);
        assert!(m1 > m0 && m2 > m1);
        // Round-trip: mel_to_hz(hz_to_mel(x)) ≈ x.
        for hz in [100.0_f32, 1000.0, 5000.0, 20000.0] {
            let round = mel_to_hz(hz_to_mel(hz));
            assert!((round - hz).abs() < 0.5, "round-trip for {} got {}", hz, round);
        }
    }

    #[test]
    fn mel_filterbank_distributes_energy() {
        // All energy in a single bin; the filterbank should have at
        // least one non-zero output.
        let mut power = vec![0.0_f32; 513];
        power[100] = 1.0;
        let filters = mel_filterbank(&power, 44100, 26);
        let total: f32 = filters.iter().sum();
        assert!(total > 0.0, "energy leaked into mel bank");
    }

    #[test]
    fn mfcc_distinguishes_different_pitches() {
        // A 500 Hz tone and a 5000 Hz tone should produce noticeably
        // different MFCC vectors.
        let sr = 44_100;
        let low  = sine(FFT_N, 500.0,  sr);
        let high = sine(FFT_N, 5000.0, sr);
        let mut win_lo = vec![0.0_f32; FFT_N];
        let mut win_hi = vec![0.0_f32; FFT_N];
        low.mono_window(0, FFT_N, &mut win_lo);
        high.mono_window(0, FFT_N, &mut win_hi);
        let mfcc_lo = compute_mfcc(&win_lo, sr);
        let mfcc_hi = compute_mfcc(&win_hi, sr);
        let dist: f32 = mfcc_lo.iter().zip(&mfcc_hi)
            .map(|(a, b)| (a - b) * (a - b)).sum::<f32>().sqrt();
        assert!(dist > 5.0, "MFCC distance too small: {}", dist);
    }

    #[test]
    fn mfcc_similar_for_same_tone() {
        let sr = 44_100;
        let a = sine(FFT_N, 1000.0, sr);
        let mut win_a = vec![0.0_f32; FFT_N];
        let mut win_b = vec![0.0_f32; FFT_N];
        a.mono_window(0, FFT_N, &mut win_a);
        a.mono_window(0, FFT_N, &mut win_b);
        let mfcc_a = compute_mfcc(&win_a, sr);
        let mfcc_b = compute_mfcc(&win_b, sr);
        let dist: f32 = mfcc_a.iter().zip(&mfcc_b)
            .map(|(a, b)| (a - b) * (a - b)).sum::<f32>().sqrt();
        assert!(dist < 0.01, "expected near-zero MFCC distance, got {}", dist);
    }

    #[test]
    fn pop_score_low_for_aligned_samples() {
        // Build a buffer where samples[end] and samples[start] are
        // identical: the loop seam between them should score low.
        // Use 441 Hz so the period (100 samples at 44.1 kHz) is an
        // exact integer — otherwise rounding the period to a whole
        // number drifts away from the real zero crossings as we
        // multiply by many cycles, producing a non-trivial pop.
        let sr = 44_100;
        let n = sr as usize * 2;        // 2 seconds
        let buf = sine(n, 441.0, sr);
        let mfcc_at = |s: usize| compute_mfcc(&{
            let mut w = vec![0.0_f32; FFT_N]; buf.mono_window(s, FFT_N, &mut w); w
        }, sr);
        let params = LoopParams::default();
        let period = (sr as f32 / 441.0) as usize;   // = 100, exact
        let start = 1024;
        let end = start + 100 * period;     // 100 full periods → same phase
        let (_total, pop, _mfcc, _ls) =
            score_candidate(&buf, start, end, &params, &mfcc_at);
        assert!(pop < 0.05, "pop score should be tiny for integer-period loop: {}", pop);
    }

    #[test]
    fn random_walk_finds_clean_loop_on_pure_sine() {
        // A pure sine wave has clean loop points at every integer
        // period — the random walk should find one and score very low.
        let sr = 22_050;
        let n = sr as usize * 5;        // 5 seconds
        let buf = sine(n, 220.0, sr);
        let params = LoopParams {
            target_seconds: 2.0,
            tolerance_seconds: 0.5,
            n_candidates: 500,
            n_refine: 4,
            n_refine_steps: 100,
            ..LoopParams::default()
        };
        let result = find_best_loop(&buf, &params, |_| {});
        // Pure sine: pop should be small after refinement.
        assert!(result.pop_score < 0.10,
            "expected clean loop, got pop={}", result.pop_score);
        // Length within tolerance.
        let dev = (result.length_seconds - 2.0).abs();
        assert!(dev < 0.6,
            "length {} not within tolerance of target 2.0", result.length_seconds);
    }

    #[test]
    fn dct_first_n_returns_expected_size() {
        let input = vec![1.0; MEL_N];
        let out = dct_first_n(&input);
        assert_eq!(out.len(), MFCC_N);
    }

    #[test]
    fn opus_output_writes_valid_ogg() {
        // Encode a 1-second sine to OggOpus and verify the file
        // starts with the canonical OggS magic, contains "OpusHead"
        // shortly after, and is non-trivially sized.
        let sr = 44_100;
        let n = sr as usize;
        let buf = sine(n, 440.0, sr);
        let tmp = std::env::temp_dir().join("looper_test_opus.opus");
        let result = LoopResult {
            start: 0,
            end: buf.n_samples() - 1,
            score: 0.0, pop_score: 0.0, mfcc_score: 0.0, length_score: 0.0,
            length_seconds: 1.0,
        };
        write_loop_opus(&buf, &result, 0, 96, &tmp).unwrap();
        let bytes = std::fs::read(&tmp).unwrap();
        // OggOpus files always start with the four-byte "OggS" page
        // sync pattern.
        assert!(bytes.starts_with(b"OggS"), "missing OggS magic");
        // The first packet (OpusHead) follows the first page header.
        // Look for the "OpusHead" magic somewhere in the first 200
        // bytes; the exact offset depends on the page header length.
        assert!(bytes[..200].windows(8).any(|w| w == b"OpusHead"),
            "OpusHead magic not found");
        // Reasonable size sanity: 1 second of audio at ~96 kbps is
        // around 12 KB. We allow a wide range to tolerate encoder
        // quality decisions.
        assert!(bytes.len() > 2_000 && bytes.len() < 50_000,
            "unexpected opus file size: {}", bytes.len());
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn opus_output_includes_opustags() {
        let sr = 44_100;
        let n = sr as usize / 2;        // 0.5 s — enough for multiple packets
        let buf = sine(n, 220.0, sr);
        let tmp = std::env::temp_dir().join("looper_test_opus_tags.opus");
        let result = LoopResult {
            start: 0,
            end: buf.n_samples() - 1,
            score: 0.0, pop_score: 0.0, mfcc_score: 0.0, length_score: 0.0,
            length_seconds: 0.5,
        };
        write_loop_opus(&buf, &result, 0, 64, &tmp).unwrap();
        let bytes = std::fs::read(&tmp).unwrap();
        // Both header packets must be present. The second header
        // ("OpusTags") sits on the second Ogg page.
        assert!(bytes.windows(8).any(|w| w == b"OpusTags"),
            "OpusTags magic not found");
        // Our vendor string identifies the encoder.
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("bo-controller/encoder"),
            "vendor string missing");
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn spectrum_average_finds_pure_tone_peak() {
        // Write 2 seconds of pure 1 kHz sine to a temp WAV, analyze,
        // expect the peak of the average spectrum near 1 kHz.
        let sr = 44_100;
        let n = sr as usize * 2;
        let tmp = std::env::temp_dir().join("spectrum_peak_test.wav");
        {
            let spec = hound::WavSpec {
                channels: 2, sample_rate: sr,
                bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(&tmp, spec).unwrap();
            for i in 0..n {
                let t = i as f32 / sr as f32;
                let v = (2.0 * PI * 1000.0 * t).sin();
                let s = (v * 16000.0) as i16;
                w.write_sample(s).unwrap();
                w.write_sample(s).unwrap();
            }
            w.finalize().unwrap();
        }

        let points = spectrum_average(&tmp, 64, 20.0, 20_000.0, 200).unwrap();
        assert_eq!(points.len(), 64);
        // Find the bin with highest dB
        let (max_idx, &(peak_hz, _peak_db)) = points.iter().enumerate()
            .max_by(|a, b| a.1.1.partial_cmp(&b.1.1).unwrap())
            .unwrap();
        // Tolerance: log-spaced bins near 1 kHz are ~140 Hz wide,
        // and Hamming windowing spreads the peak across a couple
        // of adjacent bins, so ±250 Hz of 1000 Hz is comfortable.
        assert!((peak_hz - 1000.0).abs() < 250.0,
            "expected peak near 1000 Hz, got {} Hz (bin {})",
            peak_hz, max_idx);
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn spectrum_average_rejects_too_short_files() {
        // A file with fewer samples than FFT_N must be rejected
        // with a clear error rather than silently produce garbage.
        let sr = 44_100;
        let tmp = std::env::temp_dir().join("spectrum_short_test.wav");
        {
            let spec = hound::WavSpec {
                channels: 2, sample_rate: sr,
                bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(&tmp, spec).unwrap();
            // Only 100 samples — well below FFT_N (1024).
            for _ in 0..100 {
                w.write_sample(0i16).unwrap();
                w.write_sample(0i16).unwrap();
            }
            w.finalize().unwrap();
        }
        let err = spectrum_average(&tmp, 64, 20.0, 20_000.0, 200);
        assert!(err.is_err(),
            "expected error for too-short file, got {:?}", err);
        std::fs::remove_file(&tmp).ok();
    }
}
