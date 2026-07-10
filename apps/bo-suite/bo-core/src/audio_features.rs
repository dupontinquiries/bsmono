// src/audio_features.rs
#![allow(dead_code)]
//
// Audio feature extraction for the offline CNN training pipeline.
//
// ── What this is for ──
//
// The user's hypothesis: engine noise lives largely in a HIGH-
// FREQUENCY timbre region (combustion ringing, exhaust resonance,
// valve clatter in the 500-4000 Hz band) that the fundamental-
// frequency canceller doesn't touch. A small CNN trained on this
// band could learn to recognize "this residue still sounds like an
// engine" and either gate the canceller's output or feed an
// additional cancellation/masking layer.
//
// This module is the data preparation step: it converts raw audio
// into the time-frequency representation the CNN would consume.
//
// ── Why Goertzel, not FFT ──
//
// The engine timbre band is 500-4000 Hz — that's maybe 32 frequency
// bins of interest. A full 1024-point FFT computes 512 bins, 480
// of which we throw away. Goertzel at exactly the bins we want is
// ~15× cheaper for this use case AND avoids adding an FFT library
// dependency. For full-spectrum work (e.g. mel-spectrograms) FFT
// would be the right choice; for our narrow-band timbre extraction
// Goertzel is the lighter tool.
//
// ── Output format ──
//
// `spectrogram` returns a Vec<Vec<f32>> indexed as `[time][freq]`.
// Each cell is log-magnitude (`20·log10(magnitude + ε)`) — the
// dB-scaled representation CNNs prefer for audio (raw magnitudes
// span 5+ orders of magnitude; log compression bounds the dynamic
// range to something the CNN's weights can handle).
//
// The CNN's training pipeline (Python/PyTorch, offline) would then
// stack a few rows into a time-frequency image and feed it to a
// small conv stack. We'll write that side separately when the user
// has accumulated enough captured WAVs.

use std::f32::consts::PI;

/// Configuration for one spectrogram extraction pass.
#[derive(Debug, Clone, Copy)]
pub struct SpectrogramConfig {
    /// Window length per time-frame, in samples. Typical: 1024 at
    /// 48 kHz ≈ 21 ms. Smaller = better time resolution, less
    /// frequency resolution. Larger = vice versa.
    pub window_samples: usize,
    /// Hop between consecutive frames, in samples. Typical: 512
    /// (50% overlap). Smaller = more output frames, more compute.
    pub hop_samples: usize,
    /// Lowest frequency bin in Hz.
    pub freq_lo_hz: f32,
    /// Highest frequency bin in Hz.
    pub freq_hi_hz: f32,
    /// Number of bins between lo and hi (inclusive). Linear spacing.
    /// For mel-style spacing the caller would pre-compute a custom
    /// frequency list and use `spectrogram_at_freqs` instead.
    pub n_bins: usize,
}

impl Default for SpectrogramConfig {
    fn default() -> Self {
        // Defaults tuned for engine-timbre extraction: 1024-sample
        // window (≈21 ms — short enough to track transient
        // attacks, long enough to resolve the lower harmonics);
        // 512-sample hop (50% overlap); 500-4000 Hz band with 32
        // bins (≈110 Hz spacing — covers the engine timbre region
        // without wasting bins on the cancelled fundamental).
        Self {
            window_samples: 1024,
            hop_samples: 512,
            freq_lo_hz: 500.0,
            freq_hi_hz: 4000.0,
            n_bins: 32,
        }
    }
}

/// Compute a log-magnitude spectrogram. Returns `[n_frames][n_bins]`
/// where each cell is `20·log10(magnitude + 1e-9)`.
///
/// Frames are computed by sliding `window_samples` over `samples`
/// with stride `hop_samples`. The window is rectangular (no Hann
/// taper) — that's fine for the timbre band; if you needed clean
/// nulls at specific frequencies you'd want a Hann or Hamming
/// taper, but for "what's the energy in this band" rectangular is
/// the right choice.
pub fn spectrogram(samples: &[f32], sr: f32, cfg: SpectrogramConfig) -> Vec<Vec<f32>> {
    if samples.len() < cfg.window_samples || cfg.n_bins == 0 {
        return Vec::new();
    }
    // Pre-compute the frequency list once — same for every frame.
    let freqs: Vec<f32> = (0..cfg.n_bins).map(|i| {
        if cfg.n_bins == 1 {
            (cfg.freq_lo_hz + cfg.freq_hi_hz) / 2.0
        } else {
            cfg.freq_lo_hz + (cfg.freq_hi_hz - cfg.freq_lo_hz)
                * (i as f32) / (cfg.n_bins - 1) as f32
        }
    }).collect();
    spectrogram_at_freqs(samples, sr, cfg.window_samples, cfg.hop_samples, &freqs)
}

/// Lower-level entry: compute the spectrogram at a caller-provided
/// list of frequencies. Use this for mel-style spacing or any
/// non-linear frequency layout.
pub fn spectrogram_at_freqs(
    samples: &[f32], sr: f32,
    window_samples: usize, hop_samples: usize,
    freqs: &[f32],
) -> Vec<Vec<f32>> {
    if samples.len() < window_samples || freqs.is_empty() || hop_samples == 0 {
        return Vec::new();
    }
    let n_frames = (samples.len() - window_samples) / hop_samples + 1;
    let mut out: Vec<Vec<f32>> = Vec::with_capacity(n_frames);
    for frame in 0..n_frames {
        let start = frame * hop_samples;
        let window = &samples[start..start + window_samples];
        let row: Vec<f32> = freqs.iter().map(|&f| {
            let mag = goertzel_mag(window, f, sr);
            20.0 * (mag + 1e-9).log10()
        }).collect();
        out.push(row);
    }
    out
}

/// Per-frame magnitude at one frequency. Same algorithm as the
/// passby canceller's Goertzel but inlined here so this module is
/// independent — useful for offline tooling that wants to run the
/// feature extractor without bringing in the rest of the canceller.
fn goertzel_mag(samples: &[f32], freq_hz: f32, sr: f32) -> f32 {
    if samples.is_empty() || !freq_hz.is_finite() || sr <= 0.0 {
        return 0.0;
    }
    let n = samples.len() as f32;
    let k = (0.5 + (n * freq_hz) / sr).floor();
    let omega = 2.0 * PI * k / n;
    let coef = 2.0 * omega.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &x in samples {
        let x = if x.is_finite() { x } else { 0.0 };
        let s0 = x + coef * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let real = s1 - s2 * omega.cos();
    let imag = s2 * omega.sin();
    (real * real + imag * imag).sqrt() / n
}

/// Coarse "engine timbre score" — a heuristic that pre-filters
/// audio for the CNN training set. Returns 0..1 where higher means
/// "more engine-like timbre present in this window."
///
/// The signal: real engine noise has HARMONIC structure in the
/// 500-3000 Hz band (combustion + exhaust harmonics of a fundamental
/// at 50-200 Hz). The score measures the ratio of peaks at
/// harmonically-related frequencies (relative to the windows's
/// estimated fundamental) vs. broadband energy. A pure tone gets
/// a low score (no harmonics). White noise gets a low score (no
/// peaks). A car engine should get ~0.5+.
///
/// This is NOT a CNN — it's the dumb baseline the CNN should beat.
/// Useful right now for tagging captures during the data
/// collection phase: "this 10-second window has score 0.78, almost
/// certainly an engine; this one is 0.12, probably HVAC".
pub fn engine_timbre_score(samples: &[f32], fundamental_hz: f32, sr: f32) -> f32 {
    if samples.len() < 1024 || fundamental_hz <= 0.0 { return 0.0; }
    // Look at harmonics 4 through 12 (covers 400-2400 Hz for a
    // 100 Hz fundamental, the typical engine timbre band — the
    // lower harmonics are what the canceller cancels and not the
    // timbre-identifying region).
    let mut harmonic_energy = 0.0f32;
    let mut max_freq = 0.0f32;
    for n in 4..=12 {
        let f = fundamental_hz * n as f32;
        if f >= sr * 0.45 { break; }
        harmonic_energy += goertzel_mag(samples, f, sr);
        max_freq = max_freq.max(f);
    }
    if max_freq <= 0.0 { return 0.0; }
    // Compare against broadband energy in the same range —
    // measured at frequencies that are DELIBERATELY off-harmonic
    // (between the harmonics) so we get the "non-harmonic" energy
    // in the band.
    let mut broadband_energy = 0.0f32;
    let mut count = 0;
    let lo = fundamental_hz * 4.5; // halfway between h4 and h5
    let hi = max_freq;
    let step = fundamental_hz; // sample at one-harmonic-spacing
    let mut f = lo;
    while f <= hi {
        broadband_energy += goertzel_mag(samples, f, sr);
        count += 1;
        f += step;
    }
    if count == 0 || broadband_energy < 1e-9 { return 0.0; }
    let harmonic_avg = harmonic_energy / 9.0;
    let broadband_avg = broadband_energy / count as f32;
    // Ratio score, mapped through a soft saturation so the max is
    // bounded at 1. A 4× ratio (harmonics 4× louder than between)
    // is "very engine-like" → score ≈ 0.8.
    let ratio = harmonic_avg / broadband_avg.max(1e-9);
    1.0 - (-0.3 * ratio).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs() -> f32 { 48_000.0 }

    fn synth_tone(freq: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * PI * freq * i as f32 / fs()).sin()).collect()
    }

    fn synth_engine_harmonics(fundamental: f32, n: usize) -> Vec<f32> {
        // Synthesize a "fake engine" with strong harmonics 4-10
        // (the timbre band) and weak between-harmonic content.
        (0..n).map(|i| {
            let t = i as f32 / fs();
            let mut s = 0.0;
            for h in 4..=10 {
                let f = fundamental * h as f32;
                s += (2.0 * PI * f * t).sin() * 0.3;
            }
            s
        }).collect()
    }

    #[test]
    fn empty_input_returns_empty() {
        let cfg = SpectrogramConfig::default();
        let s = spectrogram(&[], fs(), cfg);
        assert!(s.is_empty());
    }

    #[test]
    fn spectrogram_correct_dimensions() {
        let cfg = SpectrogramConfig {
            window_samples: 1024, hop_samples: 512,
            freq_lo_hz: 500.0, freq_hi_hz: 4000.0, n_bins: 32,
        };
        // 5 seconds at 48k = 240k samples. Expected frames:
        // (240k - 1024) / 512 + 1 = 466.
        let signal: Vec<f32> = vec![0.1; 240_000];
        let s = spectrogram(&signal, fs(), cfg);
        assert!(s.len() > 400 && s.len() < 500, "frame count: got {}", s.len());
        assert_eq!(s[0].len(), 32, "bin count");
    }

    #[test]
    fn pure_tone_peaks_at_its_frequency() {
        let cfg = SpectrogramConfig::default();
        let tone = synth_tone(1500.0, 4096, 1.0);
        let s = spectrogram(&tone, fs(), cfg);
        assert!(!s.is_empty());
        // The 1500 Hz bin sits at index where 500 + i*(4000-500)/31 ≈ 1500.
        // i = 31 * (1500-500)/3500 ≈ 8.85 → bin 9. The peak should
        // be near that index, not at extremes.
        let row = &s[2]; // middle frame, fully settled
        let max_idx = row.iter().enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap().0;
        assert!((max_idx as i32 - 9).abs() <= 2,
            "1500 Hz peak should be near bin 9, got bin {}", max_idx);
    }

    #[test]
    fn engine_timbre_score_high_for_harmonic_engine() {
        let signal = synth_engine_harmonics(100.0, 8192);
        let score = engine_timbre_score(&signal, 100.0, fs());
        assert!(score > 0.5,
            "synthesized harmonic engine should score >0.5, got {}", score);
    }

    #[test]
    fn engine_timbre_score_low_for_white_noise() {
        // Pseudo-random LFSR noise.
        let mut x = 12345u32;
        let signal: Vec<f32> = (0..8192).map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x as f32 / u32::MAX as f32 - 0.5) * 2.0
        }).collect();
        let score = engine_timbre_score(&signal, 100.0, fs());
        assert!(score < 0.5,
            "white noise should score <0.5, got {}", score);
    }

    #[test]
    fn engine_timbre_score_low_for_pure_tone() {
        // A single tone at 100 Hz has no upper-harmonic content,
        // so the timbre score should be low (no engine character).
        let signal = synth_tone(100.0, 8192, 1.0);
        let score = engine_timbre_score(&signal, 100.0, fs());
        assert!(score < 0.4,
            "pure tone should score <0.4, got {}", score);
    }

    #[test]
    fn spectrogram_at_freqs_uses_custom_freqs() {
        let signal = synth_tone(1000.0, 4096, 1.0);
        let custom_freqs = vec![500.0, 1000.0, 1500.0];
        let s = spectrogram_at_freqs(&signal, fs(), 1024, 512, &custom_freqs);
        assert!(!s.is_empty());
        assert_eq!(s[0].len(), 3);
        // Bin 1 (1000 Hz) should have the highest energy.
        let row = &s[2];
        assert!(row[1] > row[0] && row[1] > row[2],
            "1000 Hz bin (1) should be the peak: {:?}", row);
    }

    #[test]
    fn nan_input_does_not_propagate() {
        let mut signal = vec![0.1; 4096];
        signal[100] = f32::NAN;
        signal[200] = f32::INFINITY;
        let s = spectrogram(&signal, fs(), SpectrogramConfig::default());
        // All output values finite.
        for row in &s {
            for &v in row {
                assert!(v.is_finite(), "NaN/Inf input should not propagate");
            }
        }
    }
}
