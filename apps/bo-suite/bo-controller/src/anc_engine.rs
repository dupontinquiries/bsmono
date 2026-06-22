// src/anc_engine.rs
#![allow(dead_code)]
//
// Active noise cancellation engine.
//
// The architecture, top-to-bottom:
//
//     [mic capture] ──► push_capture() ──► [phase-aligned ring]
//                                                   │
//                                                   ▼
//                                          [PatternLearner]
//                                                   │
//                                                   ▼  (template
//                                                       generation)
//                                          [Predictor template]
//                                                   │
//                                                   ▼
//                            pull_output() ◄── [Predictor::next_sample]
//                                  │
//                                  ▼
//                  music? ──► [merge: music + α·anti-noise]
//                                  │
//                                  ▼
//                          [output device]
//
// The *separate-device* twist:
//   - The mic lives on the controller (phone). It hears ambient noise
//     PLUS whatever the speaker emits, both delayed by the round-trip
//     (DAC → amp → air → mic → ADC).
//   - The speaker is a B&O device on the LAN. The controller streams
//     anti-noise to it (or blends into a music stream). The anti-noise
//     must lead the ambient sound by the same round-trip delay if it's
//     to arrive at the listener time-aligned with the noise it cancels.
//
// We discover that delay with a *probe symbol*: a known log chirp (80-
// 200 Hz, 200 ms) played through the output and detected via cross-
// correlation in the mic capture. The peak lag of the correlation is
// the round-trip in samples. We track it across periodic re-probes
// because room temperature, listener position, and Bluetooth re-pairing
// all shift it over minutes.
//
// Honest scope:
//   - This is feed-forward predictive ANC: pattern learner extracts
//     periodic structure from the mic signal, predictor synthesizes
//     an inverse-polarity version, alignment phase-shifts it to land
//     time-aligned at the listener. It cancels what's predictable
//     (HVAC, fans, engines, road rumble). It will NOT cancel impulsive
//     or fully-stochastic noise.
//   - The "low-frequency only" focus is a real constraint: above
//     ~300 Hz the wavelength is shorter than typical listening-
//     position position uncertainty, and small head movements take the
//     listener in and out of cancellation zones. We hard-clip the
//     predictor's spectral content to ≤ 300 Hz to avoid promising
//     cancellation we can't deliver.
//   - The music-blend path is the simplest possible: sum the music
//     samples and the anti-noise samples with a configurable mix
//     ratio. No spectral masking; the listener picks the balance.
//
// Test-mode utilities at the bottom:
//   - `synth_rumble_test_signal()` builds a low-rumble + distortion
//     test signal we can play through the speaker (or push directly
//     into the engine's "synthetic ambient" port) to exercise the
//     learner + predictor + alignment loop without needing a real
//     noisy room.

use crate::predictor::{Predictor, PredictorState};
use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Sample rate the engine runs at internally. The cancel-frequency
/// hard cap of 300 Hz is well below Nyquist at 48 kHz; we standardize
/// here so all modules agree on what "1 second" of samples means.
pub const ANC_SAMPLE_RATE: u32 = 48_000;

/// Maximum frequency we'll attempt to cancel. Above this, sub-cm
/// listening-position errors take the listener out of the cancellation
/// zone — promising cancellation we can't deliver is worse than
/// admitting the limit.
pub const MAX_CANCEL_HZ: f32 = 300.0;

/// Period at which we re-run the phase-alignment probe. Long enough
/// to avoid being intrusive; short enough that drift (thermal, BT
/// pairing changes, listener movement) gets corrected within a
/// reasonable window. The actual schedule lives in the caller.
pub const ALIGNMENT_REPROBE_SECS: u64 = 90;

/// Probe chirp duration in milliseconds. 200 ms is long enough for the
/// cross-correlation peak to be sharp (lots of energy in the swept band)
/// but short enough not to be intrusive.
pub const PROBE_DURATION_MS: u32 = 200;

/// Probe chirp frequency sweep: 80 Hz → 200 Hz.
/// Below the MAX_CANCEL_HZ cap so we're characterizing the same band
/// we'll actually cancel in.
pub const PROBE_F_LO: f32 = 80.0;
pub const PROBE_F_HI: f32 = 200.0;

/// Minimum correlation strength (in normalized [0,1]) below which we
/// don't trust the measured delay — could be noise, could be the
/// device on a totally different network path. Reject and keep the
/// previous estimate.
pub const PROBE_MIN_CORRELATION: f32 = 0.15;

// ─── Phase alignment state ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct PhaseAlignment {
    /// Measured round-trip delay in samples (output emit → mic detect).
    /// `None` until first successful probe.
    pub round_trip_samples: Option<usize>,
    /// Correlation strength of last successful probe. Useful for the
    /// UI to surface "alignment confidence" to the user.
    pub last_correlation: f32,
}

impl PhaseAlignment {
    pub fn is_aligned(&self) -> bool { self.round_trip_samples.is_some() }

    /// Convert "samples we want to lead by" into "samples to delay/
    /// advance the predictor output". Returns 0 if not yet aligned —
    /// in that state the engine is still useful (the predictor cycles
    /// at the learned period) but the cancellation is non-coherent
    /// until the first probe lands.
    pub fn output_lead_samples(&self) -> usize {
        self.round_trip_samples.unwrap_or(0)
    }
}

// ─── Probe symbols ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AlignmentProbe {
    /// PCM samples to play through the output device.
    pub output_samples: Vec<f32>,
    /// Reference signal for cross-correlation in the captured stream.
    /// Same as output_samples in this simple implementation, kept
    /// separate so future probes (e.g. complementary Golay pairs)
    /// can use a different reference.
    pub reference: Vec<f32>,
    /// Sample rate (Hz).
    pub sample_rate: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct ProbeResult {
    /// Lag in samples between probe emit and probe detect = round
    /// trip delay.
    pub round_trip_samples: usize,
    /// Normalized correlation peak [0, 1]. ≥ PROBE_MIN_CORRELATION
    /// is required for the result to be trusted.
    pub correlation: f32,
}

/// Build the alignment probe — a log-sweep chirp from PROBE_F_LO to
/// PROBE_F_HI over PROBE_DURATION_MS.
///
/// Why log sweep (not linear): the cross-correlation peak is sharper
/// because the chirp's frequency content is more uniform per octave.
/// Also: a log sweep is what room-acoustics impulse-response
/// measurements use, so the result generalizes if we extend the
/// engine to characterize the room response too.
///
/// The chirp is windowed with a short cosine ramp at start and end
/// (5 ms each) so we don't click the speaker on play.
pub fn build_alignment_probe() -> AlignmentProbe {
    let sr = ANC_SAMPLE_RATE as f32;
    let n = (sr * PROBE_DURATION_MS as f32 / 1000.0) as usize;
    let mut samples = vec![0.0_f32; n];
    let f0 = PROBE_F_LO;
    let f1 = PROBE_F_HI;
    let dur = PROBE_DURATION_MS as f32 / 1000.0;
    // Log-sweep instantaneous phase:
    //   φ(t) = 2π f0 · (T/log(f1/f0)) · (exp(log(f1/f0) · t/T) - 1)
    let k = (f1 / f0).ln();
    let pre_factor = 2.0 * PI * f0 * dur / k;
    let ramp_n = ((sr * 0.005) as usize).min(n / 4); // 5 ms windowing
    for i in 0..n {
        let t = i as f32 / sr;
        let phase = pre_factor * ((k * t / dur).exp() - 1.0);
        let sample = phase.sin() * 0.5;  // 0.5 amplitude to leave headroom
        // Cosine-ramp window at start and end to elide clicks.
        let win = if i < ramp_n {
            0.5 * (1.0 - (PI * i as f32 / ramp_n as f32).cos())
        } else if i >= n - ramp_n {
            let j = (n - 1) - i;
            0.5 * (1.0 - (PI * j as f32 / ramp_n as f32).cos())
        } else { 1.0 };
        samples[i] = sample * win;
    }
    AlignmentProbe {
        output_samples: samples.clone(),
        reference: samples,
        sample_rate: ANC_SAMPLE_RATE,
    }
}

/// Cross-correlate the captured signal against a reference (the probe).
/// Returns the lag (in samples, measured from the start of `captured`)
/// at which the reference best aligns, plus the normalized correlation
/// peak.
///
/// The straightforward time-domain implementation: O(N·M) where N is
/// the captured length and M is the reference length. For 500ms of
/// capture × 200ms of reference at 48 kHz that's 24000 × 9600 ≈ 2.3
/// × 10^8 multiplies — a few hundred ms on a phone. Acceptable since
/// alignment probes run every ~90s, not per-sample.
///
/// FFT-based correlation would be ~50× faster but requires pulling in
/// rustfft as a dep; deferred until/unless probe latency matters.
pub fn cross_correlate(captured: &[f32], reference: &[f32]) -> ProbeResult {
    if captured.len() < reference.len() {
        return ProbeResult { round_trip_samples: 0, correlation: 0.0 };
    }
    let m = reference.len();
    let n = captured.len() - m + 1;

    // Pre-compute reference energy for normalization.
    let ref_energy: f32 = reference.iter().map(|x| x * x).sum::<f32>().max(1e-9).sqrt();

    let mut best_corr = 0.0_f32;
    let mut best_lag = 0_usize;
    for lag in 0..n {
        // Sanitize: a non-finite sample in either stream would propagate
        // to a non-finite correlation peak, which then beats every
        // valid alternative (NaN comparisons are false). Skip such
        // windows so the result reflects only finite measurements.
        let mut dot = 0.0_f32;
        let mut win_energy = 0.0_f32;
        let mut ok = true;
        for j in 0..m {
            let c = captured[lag + j];
            let r = reference[j];
            if !c.is_finite() || !r.is_finite() { ok = false; break; }
            dot += c * r;
            win_energy += c * c;
        }
        if !ok { continue; }
        let denom = (win_energy.max(1e-9).sqrt()) * ref_energy;
        let norm = (dot / denom).abs();  // abs: probe might be inverted by amp polarity
        if norm > best_corr {
            best_corr = norm;
            best_lag = lag;
        }
    }
    ProbeResult {
        round_trip_samples: best_lag,
        correlation: best_corr.min(1.0),
    }
}

// ─── Buffer + fill protocol ─────────────────────────────────────────────────

/// A simple lock-protected ring buffer of f32 samples. Used both for
/// capture-side input (mic samples flow in) and as the phase-aligned
/// store from which the predictor reads "past N samples" to build
/// its template.
///
/// Capacity = a few seconds worth of audio; older samples drop. The
/// engine's pattern learner reads a recent window each time it wants
/// to refresh the template.
pub struct CaptureRing {
    buf: Vec<f32>,
    head: usize,
    filled: bool,
    /// Total samples ever written. Monotonic, never wraps within a
    /// session — used by the alignment probe to position the
    /// expected detection window.
    written: AtomicUsize,
}

impl CaptureRing {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            buf: vec![0.0; capacity],
            head: 0,
            filled: false,
            written: AtomicUsize::new(0),
        }
    }

    pub fn capacity(&self) -> usize { self.buf.len() }
    pub fn written(&self) -> usize { self.written.load(Ordering::Relaxed) }

    pub fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            // Same NaN-defense pattern as mic_capture's Ring. A
            // non-finite sample from the subprocess (misaligned PCM,
            // unlikely but possible) becomes 0 here rather than
            // poisoning downstream consumers.
            let s = if s.is_finite() { s } else { 0.0 };
            self.buf[self.head] = s;
            self.head = (self.head + 1) % self.buf.len();
            if self.head == 0 { self.filled = true; }
        }
        self.written.fetch_add(samples.len(), Ordering::Relaxed);
    }

    /// Snapshot the most recent `n` samples in chronological order.
    /// Returns fewer if we haven't accumulated `n` yet.
    pub fn latest(&self, n: usize) -> Vec<f32> {
        let available = if self.filled { self.buf.len() } else { self.head };
        let take = n.min(available);
        if take == 0 { return Vec::new(); }
        let start = (self.head + self.buf.len() - take) % self.buf.len();
        let mut out = Vec::with_capacity(take);
        if start + take <= self.buf.len() {
            out.extend_from_slice(&self.buf[start..start + take]);
        } else {
            let first = self.buf.len() - start;
            out.extend_from_slice(&self.buf[start..]);
            out.extend_from_slice(&self.buf[..take - first]);
        }
        out
    }
}

// ─── ANC engine ─────────────────────────────────────────────────────────────

/// Mix ratio between the music stream and the anti-noise. The output
/// at any sample is:
///   out[i] = music[i] + α · anti[i]
/// where α is the anti-noise gain. We bound α to [0, 1] so the
/// anti-noise can never exceed unity gain (saves the speaker from a
/// runaway predictor) and provide a separate sanitize step to coerce
/// non-finite α to a safe default.
#[derive(Debug, Clone, Copy)]
pub struct MixSettings {
    pub anti_noise_gain: f32,
    pub music_gain: f32,
}

impl Default for MixSettings {
    fn default() -> Self {
        Self { anti_noise_gain: 0.6, music_gain: 1.0 }
    }
}

impl MixSettings {
    /// Coerce non-finite or out-of-range values to safe defaults.
    pub fn sanitize(self) -> Self {
        Self {
            anti_noise_gain: if self.anti_noise_gain.is_finite() {
                self.anti_noise_gain.clamp(0.0, 1.0)
            } else { 0.6 },
            music_gain: if self.music_gain.is_finite() {
                self.music_gain.clamp(0.0, 2.0)
            } else { 1.0 },
        }
    }
}

pub struct AncEngine {
    /// Predictor state — shared with whatever feeds new templates
    /// (pattern learner today, neural model later).
    predictor_state: Arc<PredictorState>,
    /// The per-stream predictor view. Wrapped in a Mutex so the
    /// pull_output entry can be called from any thread.
    predictor: Mutex<Predictor>,
    /// Capture ring fed by the mic backend.
    pub capture: Mutex<CaptureRing>,
    /// Measured round-trip delay (samples), correlation strength.
    pub alignment: Mutex<PhaseAlignment>,
    /// Whether the engine is currently emitting anti-noise. When
    /// disabled, `pull_output` returns silence (or just the music
    /// stream verbatim if a music buffer is provided).
    pub enabled: AtomicBool,
    /// Whether to blend with music (true) or play anti-noise alone
    /// (false). When false, pull_output ignores the music buffer.
    pub blend_with_music: AtomicBool,
    /// Mix ratios.
    pub mix: Mutex<MixSettings>,
}

impl AncEngine {
    /// Build a new engine. The capture ring is sized to hold 4 s of
    /// audio at the engine sample rate — enough for the pattern
    /// learner to find slow-cycle (≤ 4 s period) patterns plus headroom
    /// for the alignment probe.
    pub fn new(predictor_state: Arc<PredictorState>) -> Self {
        let capture_samples = (ANC_SAMPLE_RATE as usize) * 4;
        Self {
            predictor: Mutex::new(Predictor::new(predictor_state.clone())),
            predictor_state,
            capture: Mutex::new(CaptureRing::new(capture_samples)),
            alignment: Mutex::new(PhaseAlignment::default()),
            enabled: AtomicBool::new(false),
            blend_with_music: AtomicBool::new(true),
            mix: Mutex::new(MixSettings::default()),
        }
    }

    /// Push captured mic samples. Called from the mic-backend reader
    /// thread.
    pub fn push_capture(&self, samples: &[f32]) {
        if let Ok(mut r) = self.capture.lock() {
            r.push(samples);
        }
    }

    /// Pull `n` samples of output. If `music` is provided AND
    /// blend_with_music is true, the result is `music + α·anti_noise`;
    /// otherwise just the anti-noise (or zeros if disabled).
    ///
    /// Output is always finite (input sanitize + last-ditch check),
    /// always clipped to [-1, 1] before returning.
    pub fn pull_output(&self, n: usize, music: Option<&[f32]>) -> Vec<f32> {
        if !self.enabled.load(Ordering::Relaxed) {
            return music.map(|m| m.iter()
                .take(n)
                .map(|&v| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 })
                .collect())
                .unwrap_or_else(|| vec![0.0; n]);
        }
        let blend = self.blend_with_music.load(Ordering::Relaxed);
        let mix = self.mix.lock().map(|m| *m).unwrap_or_default().sanitize();

        let mut out = Vec::with_capacity(n);
        let mut pred = match self.predictor.lock() {
            Ok(p) => p,
            Err(_) => return music.map(|m| m.to_vec())
                .unwrap_or_else(|| vec![0.0; n]),
        };
        for i in 0..n {
            let anti = pred.next_sample();          // already in [-1, 1]
            let music_s = match (music, blend) {
                (Some(m), true) if i < m.len() => {
                    let s = m[i];
                    if s.is_finite() { s } else { 0.0 }
                }
                _ => 0.0,
            };
            let mixed = music_s * mix.music_gain + anti * mix.anti_noise_gain;
            // Final clamp protects the speaker from any combination
            // of music + anti-noise that exceeds unity. The clamp
            // accepts NaN as input (returns NaN), so guard against
            // that explicitly.
            let final_sample = if mixed.is_finite() {
                mixed.clamp(-1.0, 1.0)
            } else { 0.0 };
            out.push(final_sample);
        }
        out
    }

    /// Generate the next alignment probe. The caller plays
    /// `probe.output_samples` through the speaker at a known time T,
    /// captures the mic from T for at least
    /// `probe.output_samples.len() + max_expected_delay` samples,
    /// then passes the captured buffer to `process_probe_capture` for
    /// the lag estimate.
    pub fn build_probe(&self) -> AlignmentProbe {
        build_alignment_probe()
    }

    /// Process a probe capture. Updates `self.alignment` only if the
    /// correlation strength exceeds PROBE_MIN_CORRELATION; otherwise
    /// keeps the previous estimate.
    ///
    /// Returns the result for the caller to log / surface to the UI.
    pub fn process_probe_capture(
        &self, captured: &[f32], probe: &AlignmentProbe,
    ) -> ProbeResult {
        let result = cross_correlate(captured, &probe.reference);
        if result.correlation >= PROBE_MIN_CORRELATION {
            if let Ok(mut a) = self.alignment.lock() {
                a.round_trip_samples = Some(result.round_trip_samples);
                a.last_correlation = result.correlation;
            }
        }
        result
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    /// The current noise fundamental the engine is targeting for
    /// cancellation, in Hz. 0 when the engine has no active
    /// detection. Used by the music masker to set its comb filter's
    /// target frequency — the masker boosts the music's spectrum
    /// at exactly the band where the canceller's anti-noise leaves
    /// residue, widening the system's effective phase tolerance.
    ///
    /// Today this returns the predictor's recent template
    /// fundamental if one exists, otherwise 0. The passby /
    /// drone cancellers in standby mode have their OWN tracked
    /// fundamental; this accessor is for the play_with_anc path
    /// which uses the engine's predictor.
    pub fn tracked_noise_hz(&self) -> f32 {
        self.predictor_state.recent_fundamental_hz()
    }

    pub fn set_blend_with_music(&self, blend: bool) {
        self.blend_with_music.store(blend, Ordering::Relaxed);
    }

    pub fn set_mix(&self, mix: MixSettings) {
        if let Ok(mut m) = self.mix.lock() { *m = mix.sanitize(); }
    }

    /// Accessor for the predictor state — the pattern learner / neural
    /// model wraps this to push template updates without going through
    /// the engine API.
    pub fn predictor_state(&self) -> Arc<PredictorState> {
        self.predictor_state.clone()
    }
}

// ─── Test signal synthesis ──────────────────────────────────────────────────

/// Synthesize a low-rumble test signal with harmonic distortion.
/// Used in tests + the "what does ANC do with X?" UI experiments.
///
///   - Three sustained sines at 50 Hz (mains hum near), 80 Hz (engine-
///     like fundamental), and 120 Hz (HVAC-ish), each at decreasing
///     amplitude.
///   - Soft tanh saturation adds harmonics (the "distortion"); the
///     saturation is mild (input gain 2.5) so the fundamental still
///     dominates but second/third harmonics get audible.
///   - Slow amplitude modulation (LFO at 0.3 Hz) gives the rumble
///     non-stationarity, exercising the pattern learner's
///     adaptiveness.
///
/// Returns `n_samples` of mono PCM in [-1, 1].
pub fn synth_rumble_test_signal(n_samples: usize) -> Vec<f32> {
    let sr = ANC_SAMPLE_RATE as f32;
    let mut out = Vec::with_capacity(n_samples);
    for i in 0..n_samples {
        let t = i as f32 / sr;
        let s50  = (2.0 * PI * 50.0  * t).sin() * 0.5;
        let s80  = (2.0 * PI * 80.0  * t).sin() * 0.3;
        let s120 = (2.0 * PI * 120.0 * t).sin() * 0.15;
        let raw = s50 + s80 + s120;
        // Mild saturation — tanh(2.5x) is roughly linear for |x|<0.3
        // and rolls off smoothly above, generating mostly 2nd/3rd
        // harmonic content.
        let saturated = (2.5 * raw).tanh() * 0.5;
        // Slow LFO amplitude modulation to make the signal
        // non-stationary.
        let lfo = 0.7 + 0.3 * (2.0 * PI * 0.3 * t).sin();
        out.push(saturated * lfo);
    }
    out
}

/// As above but returns f32 + lets the caller specify which frequencies
/// to include. Useful for testing the engine against narrower target
/// bands or for matching a specific user-reported noise.
pub fn synth_custom_rumble(
    n_samples: usize, freqs_hz: &[f32], distortion: f32, lfo_hz: f32,
) -> Vec<f32> {
    let sr = ANC_SAMPLE_RATE as f32;
    let distortion = if distortion.is_finite() {
        distortion.clamp(0.0, 5.0)
    } else { 1.0 };
    let lfo_hz = if lfo_hz.is_finite() { lfo_hz.clamp(0.0, 5.0) } else { 0.3 };
    let mut out = Vec::with_capacity(n_samples);
    for i in 0..n_samples {
        let t = i as f32 / sr;
        let mut raw = 0.0_f32;
        for &f in freqs_hz {
            if !f.is_finite() || f <= 0.0 { continue; }
            // Amplitude tapers with frequency (matches typical
            // rumble's 1/f-ish spectrum).
            let amp = (50.0 / f.max(20.0)).clamp(0.05, 1.0);
            raw += (2.0 * PI * f * t).sin() * amp;
        }
        let saturated = ((1.0 + distortion) * raw).tanh() * 0.5;
        let lfo_amp = if lfo_hz > 0.0 {
            0.7 + 0.3 * (2.0 * PI * lfo_hz * t).sin()
        } else { 1.0 };
        out.push(saturated * lfo_amp);
    }
    out
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn probe_builds_in_band_chirp_of_expected_length() {
        let probe = build_alignment_probe();
        let expected = (ANC_SAMPLE_RATE as f32 * PROBE_DURATION_MS as f32 / 1000.0) as usize;
        assert_eq!(probe.output_samples.len(), expected);
        assert_eq!(probe.reference.len(), expected);
        assert_eq!(probe.sample_rate, ANC_SAMPLE_RATE);
        // Every sample finite + in bounds [-1, 1].
        for &s in &probe.output_samples {
            assert!(s.is_finite() && s.abs() <= 1.0);
        }
        // First and last samples are essentially zero (cosine ramp).
        assert!(probe.output_samples[0].abs() < 0.05);
        assert!(probe.output_samples[probe.output_samples.len() - 1].abs() < 0.05);
    }

    #[test]
    fn cross_correlate_finds_zero_lag_on_identity() {
        let probe = build_alignment_probe();
        let result = cross_correlate(&probe.output_samples, &probe.reference);
        assert_eq!(result.round_trip_samples, 0);
        assert!(result.correlation > 0.95,
            "self-correlation should be ~1, got {}", result.correlation);
    }

    #[test]
    fn cross_correlate_finds_known_lag() {
        let probe = build_alignment_probe();
        // Build a "captured" signal: zeros for 1234 samples then the
        // reference, then trailing zeros.
        let lag = 1234_usize;
        let mut captured = vec![0.0_f32; lag];
        captured.extend_from_slice(&probe.reference);
        captured.extend(vec![0.0_f32; 500]);
        let result = cross_correlate(&captured, &probe.reference);
        assert_eq!(result.round_trip_samples, lag);
        assert!(result.correlation > 0.95);
    }

    #[test]
    fn cross_correlate_robust_against_noise() {
        let probe = build_alignment_probe();
        let lag = 800_usize;
        let mut captured = vec![0.0_f32; lag];
        captured.extend_from_slice(&probe.reference);
        captured.extend(vec![0.0_f32; 500]);
        // Add light noise at -30 dB.
        let mut rng_state = 0x1234_5678_u32;
        for s in captured.iter_mut() {
            rng_state = rng_state.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = ((rng_state as f32) / (u32::MAX as f32) - 0.5) * 0.03;
            *s += n;
        }
        let result = cross_correlate(&captured, &probe.reference);
        // Allow a few-sample slop because the noise floor shifts peaks
        // slightly; we should still be within window of the true lag.
        assert!(
            (result.round_trip_samples as i64 - lag as i64).abs() < 10,
            "lag estimate {} off from true {}", result.round_trip_samples, lag);
        assert!(result.correlation > 0.5);
    }

    #[test]
    fn cross_correlate_rejects_non_finite_input() {
        let probe = build_alignment_probe();
        let mut captured = vec![0.0_f32; 200];
        captured.extend_from_slice(&probe.reference);
        // Inject NaN at a known position; the correlator should skip
        // the affected window rather than returning NaN-tainted lag.
        captured[150] = f32::NAN;
        let result = cross_correlate(&captured, &probe.reference);
        assert!(result.correlation.is_finite());
        assert!(result.round_trip_samples >= 200); // shouldn't pick lag=150
    }

    #[test]
    fn engine_disabled_emits_silence_or_music() {
        let state = Arc::new(PredictorState::new(0.5));
        let engine = AncEngine::new(state);
        // Disabled → music passes through unchanged.
        let music = vec![0.5_f32; 128];
        let out = engine.pull_output(128, Some(&music));
        for (a, b) in out.iter().zip(music.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
        // Disabled, no music → silence.
        let out = engine.pull_output(128, None);
        for v in out { assert_eq!(v, 0.0); }
    }

    #[test]
    fn engine_enabled_with_no_template_emits_music_only() {
        // Predictor with no template returns 0.0 for each next_sample;
        // the engine still mixes music in.
        let state = Arc::new(PredictorState::new(0.5));
        let engine = AncEngine::new(state);
        engine.set_enabled(true);
        let music = vec![0.3_f32; 128];
        let out = engine.pull_output(128, Some(&music));
        // music_gain=1.0, anti=0 → output = music.
        for (a, b) in out.iter().zip(music.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn engine_blends_anti_noise_with_music() {
        let state = Arc::new(PredictorState::new(1.0));
        state.update_template(vec![0.5; 64]);   // 64-sample template
        let engine = AncEngine::new(state);
        engine.set_enabled(true);
        engine.set_mix(MixSettings { anti_noise_gain: 1.0, music_gain: 1.0 });
        let music = vec![0.5_f32; 64];
        let out = engine.pull_output(64, Some(&music));
        // With template = +0.5 → anti = -0.5 × gain(1.0) = -0.5.
        // music = +0.5; mix = +0.5 + (-0.5) = 0.0.
        for v in out { assert!(v.abs() < 1e-6, "got {}", v); }
    }

    #[test]
    fn engine_blend_disabled_ignores_music() {
        let state = Arc::new(PredictorState::new(0.5));
        state.update_template(vec![0.2; 64]);
        let engine = AncEngine::new(state);
        engine.set_enabled(true);
        engine.set_blend_with_music(false);
        let music = vec![0.7_f32; 64];
        let out = engine.pull_output(64, Some(&music));
        // Blend off → music ignored. Anti per sample =
        //   -0.2 (inverse template) × 0.5 (predictor gain) = -0.1
        // then mixed at default MixSettings.anti_noise_gain = 0.6:
        //   -0.1 × 0.6 = -0.06.
        for v in out {
            assert!((v - (-0.06)).abs() < 1e-5, "got {}", v);
        }
    }

    #[test]
    fn engine_clamps_output_to_unity() {
        let state = Arc::new(PredictorState::new(1.0));
        state.update_template(vec![-1.0; 64]);  // anti will be +1.0
        let engine = AncEngine::new(state);
        engine.set_enabled(true);
        engine.set_mix(MixSettings { anti_noise_gain: 1.0, music_gain: 2.0 });
        let music = vec![1.0_f32; 64];
        let out = engine.pull_output(64, Some(&music));
        // 1.0 × 2.0 + 1.0 × 1.0 = 3.0 → clamps to 1.0.
        for v in out { assert!(v <= 1.0 + 1e-6); }
    }

    #[test]
    fn engine_mix_sanitize_handles_non_finite() {
        let mix = MixSettings { anti_noise_gain: f32::NAN, music_gain: f32::INFINITY };
        let s = mix.sanitize();
        assert!(s.anti_noise_gain.is_finite());
        assert!(s.music_gain.is_finite());
    }

    #[test]
    fn capture_ring_zero_capacity_does_not_panic() {
        // Same defense as mic_capture::Ring — coerce to capacity 1.
        let mut r = CaptureRing::new(0);
        r.push(&[1.0, 2.0]);
        let snap = r.latest(10);
        assert_eq!(snap.len(), 1);
    }

    #[test]
    fn capture_ring_sanitizes_non_finite_input() {
        let mut r = CaptureRing::new(8);
        r.push(&[1.0, f32::NAN, 2.0, f32::INFINITY]);
        let snap = r.latest(4);
        for v in &snap { assert!(v.is_finite()); }
        assert_eq!(snap, vec![1.0, 0.0, 2.0, 0.0]);
    }

    #[test]
    fn engine_push_capture_updates_ring() {
        let state = Arc::new(PredictorState::new(0.5));
        let engine = AncEngine::new(state);
        let samples = vec![0.3_f32; 100];
        engine.push_capture(&samples);
        let written = engine.capture.lock().unwrap().written();
        assert_eq!(written, 100);
    }

    #[test]
    fn probe_capture_below_threshold_does_not_update_alignment() {
        let state = Arc::new(PredictorState::new(0.5));
        let engine = AncEngine::new(state);
        let probe = engine.build_probe();
        // Pure noise — correlation will be low.
        let mut rng = 0x_abcd_1234_u32;
        let mut captured = vec![0.0_f32; probe.reference.len() * 3];
        for s in captured.iter_mut() {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            *s = ((rng as f32) / (u32::MAX as f32) - 0.5) * 0.1;
        }
        let result = engine.process_probe_capture(&captured, &probe);
        assert!(result.correlation < PROBE_MIN_CORRELATION);
        // alignment.round_trip_samples should still be None.
        assert!(engine.alignment.lock().unwrap().round_trip_samples.is_none());
    }

    #[test]
    fn probe_capture_above_threshold_updates_alignment() {
        let state = Arc::new(PredictorState::new(0.5));
        let engine = AncEngine::new(state);
        let probe = engine.build_probe();
        // Synthesized "captured" signal with known lag.
        let known_lag = 567_usize;
        let mut captured = vec![0.0_f32; known_lag];
        captured.extend_from_slice(&probe.reference);
        captured.extend(vec![0.0_f32; 200]);
        let result = engine.process_probe_capture(&captured, &probe);
        assert!(result.correlation >= PROBE_MIN_CORRELATION);
        let align = engine.alignment.lock().unwrap();
        assert_eq!(align.round_trip_samples, Some(known_lag));
        assert!(align.is_aligned());
    }

    #[test]
    fn rumble_signal_is_in_unity_range_and_finite() {
        let s = synth_rumble_test_signal(48_000);  // 1s
        for v in &s {
            assert!(v.is_finite());
            assert!(v.abs() <= 1.0);
        }
        // Has measurable energy.
        let rms = (s.iter().map(|x| x*x).sum::<f32>() / s.len() as f32).sqrt();
        assert!(rms > 0.05);
    }

    #[test]
    fn custom_rumble_with_empty_freqs_is_silent() {
        let s = synth_custom_rumble(1000, &[], 1.0, 0.3);
        for v in s { assert!(v.abs() < 1e-6); }
    }

    #[test]
    fn custom_rumble_handles_non_finite_params() {
        let s = synth_custom_rumble(
            100, &[50.0, f32::NAN, 80.0, f32::INFINITY], f32::NAN, f32::INFINITY);
        for v in s {
            assert!(v.is_finite());
            assert!(v.abs() <= 1.0);
        }
    }
}
