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
// ── Repointed onto audio-kit ──
//
// `SpectrogramConfig`, `spectrogram`, and `spectrogram_at_freqs` used
// to be a full local implementation — byte-for-byte identical to what
// is now `audio_kit::goertzel` (that module's own doc: "Lifted from
// bo-suite's audio_features... copied verbatim across bo-mux and
// bo-controller — the largest single duplication the clone scan
// found"). This module is now a thin re-export of those three, kept
// under the SAME names so `bo-controller`'s `pub use bo_core::
// audio_features;` and every existing call site need no changes.
//
// `engine_timbre_score` stays HERE, local, on purpose — it's a
// strategy built on top of the Goertzel primitive (which harmonics to
// look at, how to score them for "engine-ness"), not a DSP primitive
// itself, and audio-kit's own scope is leaf building blocks, not
// app-specific heuristics. It now calls `audio_kit::goertzel::
// goertzel` instead of a local `goertzel_mag` — same function, no
// behavior change.
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
        harmonic_energy += audio_kit::goertzel::goertzel(samples, f, sr);
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
        broadband_energy += audio_kit::goertzel::goertzel(samples, f, sr);
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
    use std::f32::consts::PI;

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

    // The re-exported spectrogram/SpectrogramConfig already have their own
    // tests in audio-kit — these two are a thin "the shim actually wires
    // up" sanity check, not a re-test of the algorithm itself.
    #[test]
    fn spectrogram_reexport_still_works_through_this_module() {
        let cfg = SpectrogramConfig::default();
        assert!(spectrogram(&[], fs(), cfg).is_empty());
        let tone = synth_tone(1500.0, 4096, 1.0);
        let s = spectrogram(&tone, fs(), cfg);
        assert!(!s.is_empty());
    }

    #[test]
    fn spectrogram_at_freqs_reexport_still_works_through_this_module() {
        let signal = synth_tone(1000.0, 4096, 1.0);
        let s = spectrogram_at_freqs(&signal, fs(), 1024, 512, &[500.0, 1000.0, 1500.0]);
        assert!(!s.is_empty());
        assert_eq!(s[0].len(), 3);
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
}
