//! Audio feature extraction — now a thin shim over `audio-kit`.
//!
//! The DSP primitives (Goertzel magnitude and the spectrogram builders) moved to
//! `audio_kit::goertzel`, which lifted them verbatim from this file — the two
//! byte-identical copies in bo-mux and bo-controller are exactly what motivated
//! the kit. This module re-exports them and keeps `engine_timbre_score`, the one
//! bo-suite-specific scorer, local — now calling audio-kit's Goertzel.

pub use audio_kit::goertzel::{spectrogram, spectrogram_at_freqs, SpectrogramConfig};

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
    use audio_kit::goertzel::goertzel as goertzel_mag;
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
