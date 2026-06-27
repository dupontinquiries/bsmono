//! goertzel.rs — single-frequency magnitude (the Goertzel algorithm) and a
//! Goertzel-based narrow-band spectrogram. Lifted from bo-suite's `audio_features`
//! (copied verbatim across bo-mux and bo-controller — the largest single
//! duplication the clone scan found).
//!
//! Goertzel computes the magnitude at EXACTLY the frequencies you ask for, which
//! is far cheaper than a full FFT when you only care about a handful of bins —
//! tone detection, DTMF, or a narrow timbre band (the engine-noise use it came
//! from looked at ~32 bins in 500–4000 Hz; an FFT would have computed 512 and
//! thrown most away). For full-spectrum work (mel-spectrograms) an FFT is the
//! right tool; for narrow-band magnitude this is the lighter one. NaN/Inf-safe.

use std::f32::consts::PI;

/// Magnitude of `samples` at `freq_hz`, via the Goertzel algorithm. Returns `0.0`
/// for degenerate input (empty buffer, non-finite frequency, non-positive sample
/// rate); non-finite samples are treated as `0`.
pub fn goertzel(samples: &[f32], freq_hz: f32, sr: f32) -> f32 {
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

/// Configuration for one spectrogram extraction pass.
#[derive(Debug, Clone, Copy)]
pub struct SpectrogramConfig {
    /// Window length per time-frame, in samples (e.g. 1024 at 48 kHz ≈ 21 ms).
    /// Smaller = better time resolution; larger = better frequency resolution.
    pub window_samples: usize,
    /// Hop between consecutive frames, in samples (e.g. 512 = 50% overlap).
    pub hop_samples: usize,
    /// Lowest frequency bin in Hz.
    pub freq_lo_hz: f32,
    /// Highest frequency bin in Hz.
    pub freq_hi_hz: f32,
    /// Number of linearly-spaced bins between lo and hi (inclusive). For
    /// mel-style spacing, pre-compute a frequency list and use
    /// [`spectrogram_at_freqs`].
    pub n_bins: usize,
}

impl Default for SpectrogramConfig {
    fn default() -> Self {
        Self {
            window_samples: 1024,
            hop_samples: 512,
            freq_lo_hz: 500.0,
            freq_hi_hz: 4000.0,
            n_bins: 32,
        }
    }
}

/// Compute a log-magnitude spectrogram, `[n_frames][n_bins]`, where each cell is
/// `20·log10(magnitude + 1e-9)`. Frames slide `window_samples` over `samples`
/// with stride `hop_samples`; the window is rectangular (fine for band energy —
/// for clean nulls at specific frequencies apply a [`crate::window`] taper first).
pub fn spectrogram(samples: &[f32], sr: f32, cfg: SpectrogramConfig) -> Vec<Vec<f32>> {
    if samples.len() < cfg.window_samples || cfg.n_bins == 0 {
        return Vec::new();
    }
    // Pre-compute the frequency list once — same for every frame.
    let freqs: Vec<f32> = (0..cfg.n_bins)
        .map(|i| {
            if cfg.n_bins == 1 {
                (cfg.freq_lo_hz + cfg.freq_hi_hz) / 2.0
            } else {
                cfg.freq_lo_hz
                    + (cfg.freq_hi_hz - cfg.freq_lo_hz) * (i as f32) / (cfg.n_bins - 1) as f32
            }
        })
        .collect();
    spectrogram_at_freqs(samples, sr, cfg.window_samples, cfg.hop_samples, &freqs)
}

/// Lower-level entry: compute the spectrogram at a caller-provided list of
/// frequencies. Use this for mel-style spacing or any non-linear layout.
pub fn spectrogram_at_freqs(
    samples: &[f32],
    sr: f32,
    window_samples: usize,
    hop_samples: usize,
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
        let row: Vec<f32> = freqs
            .iter()
            .map(|&f| {
                let mag = goertzel(window, f, sr);
                20.0 * (mag + 1e-9).log10()
            })
            .collect();
        out.push(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs() -> f32 {
        48_000.0
    }

    fn synth_tone(freq: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * PI * freq * i as f32 / fs()).sin()).collect()
    }

    #[test]
    fn goertzel_peaks_at_the_tone_frequency() {
        let tone = synth_tone(1000.0, 4096, 1.0);
        let on = goertzel(&tone, 1000.0, fs());
        let off = goertzel(&tone, 1700.0, fs());
        assert!(on > off * 5.0, "energy concentrates at the tone: on={on} off={off}");
    }

    #[test]
    fn goertzel_defends_against_non_finite() {
        assert_eq!(goertzel(&[], 1000.0, fs()), 0.0);
        assert_eq!(goertzel(&[0.1; 64], f32::NAN, fs()), 0.0);
        assert_eq!(goertzel(&[0.1; 64], 1000.0, 0.0), 0.0);
        let mut sig = vec![0.1; 256];
        sig[10] = f32::NAN;
        sig[20] = f32::INFINITY;
        assert!(goertzel(&sig, 1000.0, fs()).is_finite());
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(spectrogram(&[], fs(), SpectrogramConfig::default()).is_empty());
    }

    #[test]
    fn spectrogram_correct_dimensions() {
        let cfg = SpectrogramConfig::default();
        // (240k - 1024) / 512 + 1 = 466 frames; 32 bins.
        let signal: Vec<f32> = vec![0.1; 240_000];
        let s = spectrogram(&signal, fs(), cfg);
        assert!(s.len() > 400 && s.len() < 500, "frame count: got {}", s.len());
        assert_eq!(s[0].len(), 32, "bin count");
    }

    #[test]
    fn pure_tone_peaks_at_its_frequency_bin() {
        let tone = synth_tone(1500.0, 4096, 1.0);
        let s = spectrogram(&tone, fs(), SpectrogramConfig::default());
        assert!(!s.is_empty());
        // 500 + i*(4000-500)/31 ≈ 1500 → bin ≈ 9.
        let row = &s[2];
        let max_idx = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert!((max_idx as i32 - 9).abs() <= 2, "1500 Hz peak near bin 9, got {max_idx}");
    }

    #[test]
    fn spectrogram_at_freqs_uses_custom_freqs() {
        let signal = synth_tone(1000.0, 4096, 1.0);
        let s = spectrogram_at_freqs(&signal, fs(), 1024, 512, &[500.0, 1000.0, 1500.0]);
        assert!(!s.is_empty());
        assert_eq!(s[0].len(), 3);
        let row = &s[2];
        assert!(row[1] > row[0] && row[1] > row[2], "1000 Hz bin is the peak: {row:?}");
    }

    #[test]
    fn nan_input_does_not_propagate() {
        let mut signal = vec![0.1; 4096];
        signal[100] = f32::NAN;
        signal[200] = f32::INFINITY;
        let s = spectrogram(&signal, fs(), SpectrogramConfig::default());
        for row in &s {
            for &v in row {
                assert!(v.is_finite(), "NaN/Inf input must not propagate");
            }
        }
    }
}
