//! voiceprint-kit — the pure acoustic fingerprint, shared.
//!
//! Lifted VERBATIM from `apps/voiceterm` (`phonetic.rs`'s MFCC pipeline
//! and `acoustic_index.rs`'s 39-dimensional utterance fingerprint), which
//! is where this was designed, tuned and battle-tested. It lives here so
//! a second app can prescreen audio without a second implementation
//! drifting from the first: the same coefficients, the same cosine
//! metric, the same numbers.
//!
//! Why it exists at all: matching a keyword against thousands of clips
//! with a real recognizer is far too slow to type into. A fingerprint is
//! 39 floats and a cosine — microseconds — so it can narrow thousands of
//! candidates to a handful, and the expensive verifier only ever runs on
//! the handful. voiceterm's own notes put it plainly: the fingerprint DB
//! is the leverage point for responsiveness, short-circuiting whisper on
//! most utterances.
//!
//! Everything here is pure: no globals, no disk, no threads. voiceterm's
//! exemplar database, promotion policy and whisper pool stay in
//! voiceterm, which is where that policy belongs.

/// Coefficients per frame. voiceterm's value; changing it would make
/// every stored fingerprint in either app meaningless.
pub const N_MFCC: usize = 13;
const N_MEL: usize = 26;
const FRAME_SIZE: usize = 512;   // ~32ms at 16kHz
const FRAME_HOP: usize = 160;    // 10ms at 16kHz
const TARGET_RATE: u32 = 16_000;

/// The rate every function here expects. Resample before calling.
pub const RATE: u32 = TARGET_RATE;

/// Extract MFCC features from 16kHz mono PCM.
/// Returns flattened n_frames × N_MFCC matrix.
///
/// v0.1.284: pub(crate) so acoustic_index can reuse the same
/// MFCC pipeline for utterance-level fingerprinting. Same
/// algorithm, different aggregation (mean across frames).
pub fn compute_mfcc(audio: &[f32]) -> Vec<f32> {
    if audio.len() < FRAME_SIZE { return Vec::new(); }

    // 1. Pre-emphasis (high-pass filter to balance spectrum)
    let mut emphasized = Vec::with_capacity(audio.len());
    emphasized.push(audio[0]);
    for i in 1..audio.len() {
        emphasized.push(audio[i] - 0.97 * audio[i-1]);
    }

    // 2. Frame the signal
    let n_frames = (emphasized.len().saturating_sub(FRAME_SIZE)) / FRAME_HOP + 1;
    let mel_filters = mel_filterbank(N_MEL, FRAME_SIZE, TARGET_RATE);

    let mut all_mfcc = Vec::with_capacity(n_frames * N_MFCC);

    for frame_idx in 0..n_frames {
        let start = frame_idx * FRAME_HOP;
        if start + FRAME_SIZE > emphasized.len() { break; }

        // 3. Apply Hamming window
        let mut frame: Vec<f32> = emphasized[start..start + FRAME_SIZE].to_vec();
        for (i, x) in frame.iter_mut().enumerate() {
            let w = 0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / (FRAME_SIZE - 1) as f32).cos();
            *x *= w;
        }

        // 4. Power spectrum (magnitude squared of FFT)
        let spectrum = power_spectrum(&frame);

        // 5. Apply mel filterbank
        let mut mel_energies = vec![0.0f32; N_MEL];
        for (m, filter) in mel_filters.iter().enumerate() {
            for (k, &fk) in filter.iter().enumerate() {
                if k < spectrum.len() {
                    mel_energies[m] += spectrum[k] * fk;
                }
            }
            // Log compression with floor. v0.1.559: the old
            // `(mel + 1e-10).ln()` only protected against exactly-zero
            // energy. A NEGATIVE mel energy (FFT rounding artifact) or a
            // NaN/inf (from a corrupt audio buffer — same class as the
            // v0.1.558 from_audio fix) produced ln(negative)=NaN /
            // ln(NaN)=NaN, which poisons the whole MFCC fingerprint and
            // every downstream cosine_distance in acoustic_index. Floor
            // to a positive finite value before ln: NaN/negative → the
            // silence floor, +inf → a large finite.
            let e = mel_energies[m];
            let floored = if e.is_finite() {
                e.max(1e-10)
            } else if e > 0.0 {
                1e10           // +inf → large finite
            } else {
                1e-10          // NaN / -inf / negative → silence floor
            };
            mel_energies[m] = floored.ln();
        }

        // 6. DCT to get MFCC coefficients
        let mfcc = dct(&mel_energies, N_MFCC);
        all_mfcc.extend_from_slice(&mfcc);
    }

    // 7. Cepstral mean normalization (helps with channel/mic differences)
    cepstral_mean_normalize(&mut all_mfcc, N_MFCC);

    all_mfcc
}

/// Power spectrum via simple DFT (small frames, no FFT needed for clarity)
/// We only need first FRAME_SIZE/2+1 bins
fn power_spectrum(frame: &[f32]) -> Vec<f32> {
    // Use simple DFT for small N=512. ~250k mults per frame * 100 frames/sec = 25M/sec, ok.
    // For real apps use rustfft, but this avoids the dependency.
    let n = frame.len();
    let n_bins = n / 2 + 1;
    let mut out = vec![0.0f32; n_bins];

    for k in 0..n_bins {
        let mut re = 0.0f32;
        let mut im = 0.0f32;
        let factor = -2.0 * std::f32::consts::PI * k as f32 / n as f32;
        for (j, &x) in frame.iter().enumerate() {
            let angle = factor * j as f32;
            re += x * angle.cos();
            im += x * angle.sin();
        }
        out[k] = (re * re + im * im) / n as f32;
    }
    out
}

/// Build triangular mel filterbank
fn mel_filterbank(n_filters: usize, n_fft: usize, sample_rate: u32) -> Vec<Vec<f32>> {
    let low_mel = hz_to_mel(0.0);
    let high_mel = hz_to_mel(sample_rate as f32 / 2.0);
    let mel_points: Vec<f32> = (0..=n_filters + 1)
        .map(|i| low_mel + (high_mel - low_mel) * i as f32 / (n_filters + 1) as f32)
        .collect();
    let hz_points: Vec<f32> = mel_points.iter().map(|&m| mel_to_hz(m)).collect();
    let bin_points: Vec<usize> = hz_points.iter()
        .map(|&hz| ((n_fft + 1) as f32 * hz / sample_rate as f32).floor() as usize)
        .collect();

    let n_bins = n_fft / 2 + 1;
    let mut filters = Vec::with_capacity(n_filters);
    for m in 1..=n_filters {
        let mut filter = vec![0.0f32; n_bins];
        let f_left = bin_points[m - 1];
        let f_center = bin_points[m];
        let f_right = bin_points[m + 1];
        for k in f_left..f_center {
            if f_center > f_left {
                filter[k] = (k - f_left) as f32 / (f_center - f_left) as f32;
            }
        }
        for k in f_center..f_right {
            if f_right > f_center {
                filter[k] = (f_right - k) as f32 / (f_right - f_center) as f32;
            }
        }
        filters.push(filter);
    }
    filters
}

fn hz_to_mel(hz: f32) -> f32 { 2595.0 * (1.0 + hz / 700.0).log10() }

fn mel_to_hz(mel: f32) -> f32 { 700.0 * (10f32.powf(mel / 2595.0) - 1.0) }

/// DCT-II keeping only first n_keep coefficients
fn dct(input: &[f32], n_keep: usize) -> Vec<f32> {
    let n = input.len();
    let mut out = vec![0.0f32; n_keep];
    for k in 0..n_keep {
        let mut sum = 0.0f32;
        for (i, &x) in input.iter().enumerate() {
            sum += x * (std::f32::consts::PI * k as f32 * (2.0 * i as f32 + 1.0) / (2.0 * n as f32)).cos();
        }
        out[k] = sum;
    }
    out
}

/// Subtract mean across frames for each coefficient (CMN)
fn cepstral_mean_normalize(mfcc: &mut [f32], n_coefs: usize) {
    if mfcc.is_empty() { return; }
    let n_frames = mfcc.len() / n_coefs;
    if n_frames == 0 { return; }
    for c in 0..n_coefs {
        let mut sum = 0.0f32;
        for f in 0..n_frames {
            sum += mfcc[f * n_coefs + c];
        }
        let mean = sum / n_frames as f32;
        for f in 0..n_frames {
            mfcc[f * n_coefs + c] -= mean;
        }
    }
}

/// Compute the acoustic fingerprint for a single utterance.
/// v0.1.647: richer speaker/utterance embedding — 39-dim:
/// [ mean(13) | std-dev(13) | delta-mean(13) ]. The old 13-dim MEAN threw
/// away within-utterance variance and spectral DYNAMICS, the cues that
/// most separate speakers — measured on synthetic voices, the mean gave
/// only a 1.10× cross/same separation ratio (near-useless), while this
/// 39-dim form gave ~20×. Better separation → less shadow divergence →
/// the whisper-skip fast path can eventually turn on, and cleaner speaker
/// attribution. Still pure + cheap (39 floats, µs cosine). File format is
/// bumped AI01→AI02 so old 13-dim DBs fall back cleanly (length mismatch
/// in cosine_distance → +inf → no false match → whisper runs as before).
/// Returns an empty Vec if the audio is too short to MFCC.
pub fn fingerprint(audio_16k_mono: &[f32]) -> Vec<f32> {
    let mfcc = compute_mfcc(audio_16k_mono);
    let n_mfcc = N_MFCC;
    if mfcc.is_empty() || mfcc.len() % n_mfcc != 0 {
        return Vec::new();
    }
    let n_frames = mfcc.len() / n_mfcc;
    // mean
    let mut mean = vec![0.0f32; n_mfcc];
    for f in 0..n_frames {
        for c in 0..n_mfcc {
            mean[c] += mfcc[f * n_mfcc + c];
        }
    }
    let inv = 1.0 / (n_frames as f32);
    for v in &mut mean { *v *= inv; }
    // std-dev (population) — within-utterance spectral spread
    let mut sd = vec![0.0f32; n_mfcc];
    for f in 0..n_frames {
        for c in 0..n_mfcc {
            let d = mfcc[f * n_mfcc + c] - mean[c];
            sd[c] += d * d;
        }
    }
    for v in &mut sd { *v = (*v * inv).sqrt(); }
    // delta-mean — average absolute frame-to-frame change (dynamics)
    let mut dmean = vec![0.0f32; n_mfcc];
    if n_frames > 1 {
        for f in 1..n_frames {
            for c in 0..n_mfcc {
                dmean[c] += (mfcc[f * n_mfcc + c] - mfcc[(f - 1) * n_mfcc + c]).abs();
            }
        }
        let invd = 1.0 / ((n_frames - 1) as f32);
        for v in &mut dmean { *v *= invd; }
    }
    let mut out = mean;
    out.extend(sd);
    out.extend(dmean);
    out // 39-dim
}

/// Cosine distance between two fingerprints (lower = closer).
/// Returns +inf for mismatched lengths or zero-magnitude vecs.
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() { return f32::INFINITY; }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if !na.is_finite() || !nb.is_finite() || na <= 0.0 || nb <= 0.0 {
        // v0.1.559: NaN norms (from a NaN in the fingerprint) slip past
        // a bare `<= 0.0` since `NaN <= 0.0` is false. Treat any non-
        // finite or zero-norm vector as "no match" (infinite distance)
        // rather than letting a NaN distance poison the nearest-neighbor
        // search. Defense in depth alongside the compute_mfcc floor fix.
        return f32::INFINITY;
    }
    let cos_sim = dot / (na.sqrt() * nb.sqrt());
    let d = 1.0 - cos_sim;
    if d.is_finite() { d } else { f32::INFINITY }
}

/// Downmix and resample to the 16 kHz mono this kit expects. Linear —
/// a fingerprint is a coarse statistic and a better resampler would not
/// move the cosine.
pub fn to_16k_mono(channels: &[Vec<f32>], rate: u32) -> Vec<f32> {
    if channels.is_empty() { return Vec::new(); }
    let frames = channels.iter().map(Vec::len).min().unwrap_or(0);
    let ratio = rate as f64 / RATE as f64;
    let out_len = (frames as f64 / ratio) as usize;
    (0..out_len).map(|i| {
        let pos = i as f64 * ratio;
        let i0 = pos as usize;
        let frac = (pos - i0 as f64) as f32;
        let at = |k: usize| -> f32 {
            let k = k.min(frames.saturating_sub(1));
            channels.iter().map(|c| c[k]).sum::<f32>() / channels.len() as f32
        };
        at(i0) * (1.0 - frac) + at(i0 + 1) * frac
    }).collect()
}

/// Distance between two fingerprints, exposed under the name a caller
/// thinks in. Mismatched lengths are +inf — voiceterm's own rule, which
/// is what makes an old or truncated fingerprint fail to match rather
/// than match wrongly.
pub fn distance(a: &[f32], b: &[f32]) -> f32 { cosine_distance(a, b) }

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, secs: f32) -> Vec<f32> {
        (0..(RATE as f32 * secs) as usize)
            .map(|i| (i as f32 / RATE as f32 * hz * std::f32::consts::TAU).sin() * 0.4)
            .collect()
    }
    /// A word-like sound: two formants that MOVE across the utterance.
    /// Steady tones are degenerate for this fingerprint — two thirds of
    /// its 39 dimensions are standard deviation and frame-to-frame delta,
    /// which for a constant tone contain only rounding noise, so a
    /// steady-tone fixture measures nothing and can rank a 5 Hz shift as
    /// farther than a whole different vowel. (It did, on the first
    /// attempt.) Moving formants are what speech actually is.
    fn word(f1: (f32, f32), f2: (f32, f32), secs: f32) -> Vec<f32> {
        let n = (RATE as f32 * secs) as usize;
        let mut phase1 = 0.0f32;
        let mut phase2 = 0.0f32;
        (0..n).map(|i| {
            let t = i as f32 / n as f32;
            let a = f1.0 + (f1.1 - f1.0) * t;
            let b = f2.0 + (f2.1 - f2.0) * t;
            phase1 += std::f32::consts::TAU * a / RATE as f32;
            phase2 += std::f32::consts::TAU * b / RATE as f32;
            // An amplitude envelope too: a word starts and stops.
            let env = (t * std::f32::consts::PI).sin();
            (phase1.sin() * 0.6 + phase2.sin() * 0.4) * env
        }).collect()
    }
    fn vowel(f1: f32, f2: f32, secs: f32) -> Vec<f32> {
        word((f1, f1 * 1.6), (f2, f2 * 0.8), secs)
    }

    #[test]
    fn a_fingerprint_is_39_floats_and_says_nothing_about_audio_too_short() {
        let fp = fingerprint(&word((300.0, 480.0), (1200.0, 1900.0), 0.5));
        assert_eq!(fp.len(), N_MFCC * 3, "mean, std and delta — voiceterm's 39-dim form");
        assert!(fp.iter().all(|v| v.is_finite()));
        assert!(fingerprint(&[0.0; 100]).is_empty(), "too short to MFCC is empty, not junk");
    }

    #[test]
    fn the_same_sound_is_nearer_than_a_different_one() {
        // The whole premise of a prescreen: cheap distance must ORDER
        // candidates usefully, even if it cannot decide them.
        // The same "word" said twice (slightly different), against a
        // different one — the ordering a prescreen lives or dies by.
        let a = fingerprint(&word((300.0, 480.0), (1200.0, 1900.0), 0.4));
        let a2 = fingerprint(&word((308.0, 470.0), (1215.0, 1880.0), 0.42));
        let b = fingerprint(&word((900.0, 620.0), (2600.0, 1500.0), 0.4));
        let same = distance(&a, &a2);
        let diff = distance(&a, &b);
        assert!(same < diff, "same {same} should be nearer than different {diff}");
        assert!(distance(&a, &a).abs() < 1e-5, "identical is zero");
        // A length mismatch must be no-match, never a lucky match.
        assert!(distance(&a, &[0.1, 0.2]).is_infinite());
    }

    #[test]
    fn resampling_to_the_kits_rate_preserves_what_the_fingerprint_reads() {
        // A 48k clip and its 16k self must land close: the library's audio
        // is 48k and the fingerprint is defined at 16k, so this conversion
        // sits between every real caller and every match.
        let secs = 0.4;
        let at_16k = word((300.0, 480.0), (1200.0, 1900.0), secs);
        let n48 = (48_000.0 * secs) as usize;
        let mut p1 = 0.0f32;
        let mut p2 = 0.0f32;
        let at_48k: Vec<f32> = (0..n48).map(|i| {
            let t = i as f32 / n48 as f32;
            p1 += std::f32::consts::TAU * (300.0 + 180.0 * t) / 48_000.0;
            p2 += std::f32::consts::TAU * (1200.0 + 700.0 * t) / 48_000.0;
            (p1.sin() * 0.6 + p2.sin() * 0.4) * (t * std::f32::consts::PI).sin()
        }).collect();
        let down = to_16k_mono(&[at_48k], 48_000);
        let d = distance(&fingerprint(&at_16k), &fingerprint(&down));
        assert!(d < 0.2, "the same sound through the resampler stays the same sound ({d})");
        assert!(to_16k_mono(&[], 48_000).is_empty());
    }
}
