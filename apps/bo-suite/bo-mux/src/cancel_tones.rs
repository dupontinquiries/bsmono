// src/cancel_tones.rs
//
// Phase-tracked anti-noise tone generator. Produces a fundamental sinusoid
// at a user-specified frequency, plus harmonics with a 1/n rolloff. The
// output is intended to be summed with the noise masker and played
// continuously — superposing with whatever environmental rumble the user
// is trying to suppress.
//
// HONEST SCOPE (this is important, don't skip):
//
// True active noise cancellation requires a microphone feedback loop so the
// system can measure the actual noise at the listener's ear and produce its
// inverse in real time. We have neither a mic nor low-latency speaker
// access in this setup, so we CAN'T do real ANC.
//
// What we CAN do is play coherent tones at the noise's fundamental
// frequency and rely on two well-studied effects:
//
//   1. Psychoacoustic masking: adding tonal energy at the noise's
//      frequencies raises the masking threshold there. The auditory system
//      stops resolving the noise as a separate sound — it gets pulled into
//      the "constant background" pool and attention shifts away. Over a
//      few minutes this measurably reduces *perceived* loudness, even
//      though the physical SPL hasn't moved.
//
//   2. Lucky-phase destructive interference: continuous environmental
//      noise (refrigerator compressors, engine drone at fixed RPM, HVAC
//      blowers) is highly periodic over short timescales. If the user
//      tunes our anti-tone to closely match the noise's fundamental, we
//      WILL be 180° out of phase for ~half the time, producing real
//      cancellation in those windows. On average over an hour, the effect
//      is small — but combined with (1), it's a noticeable improvement
//      for the right kinds of noise.
//
// What this does NOT work well for:
//   - Transient noise (door slams, traffic spikes, voice)
//   - Broadband noise without a clear fundamental (waterfall)
//   - High-frequency noise (>500 Hz; wavelength too short for room-scale
//     phase coherence)
//
// The user controls a single fundamental frequency and the system generates
// N harmonics with 1/n amplitude rolloff (which matches the natural
// harmonic structure of engine drone, compressor whine, etc).

use std::f32::consts::PI;

/// Generator for an inverse-phase tone train: fundamental + harmonics,
/// each rolled off as 1/n. State is a per-harmonic phase accumulator.
pub struct AntiToneGen {
    sample_rate: f32,
    fundamental_hz: f32,
    phases: Vec<f32>,
    amplitudes: Vec<f32>,
}

impl AntiToneGen {
    /// Create a generator with `n_harmonics` total (fundamental counts as
    /// harmonic 1). When `inverse=true`, each tone starts at 180° offset
    /// so the running sum begins out-of-phase relative to a same-frequency
    /// noise that started at t=0. With a free-running noise source the
    /// actual phase relationship drifts, but starting inverse is the best
    /// bet without feedback information.
    pub fn new(sample_rate: u32, fundamental_hz: f32, n_harmonics: usize, inverse: bool) -> Self {
        let n_harmonics = n_harmonics.max(1).min(16);
        let amplitudes: Vec<f32> = (1..=n_harmonics).map(|n| 1.0 / n as f32).collect();
        let initial = if inverse { PI } else { 0.0 };
        let phases = vec![initial; n_harmonics];
        // NaN defense: f32::max(NaN, 1.0) returns NaN, not 1.0 —
        // because partial_cmp returns None on NaN and Rust's max
        // falls through to the input value. A NaN fundamental_hz
        // would then propagate into every phase increment, every
        // phase.sin(), and ultimately silence the sanitizer with
        // no diagnostic. Sister fix already lives in bo-controller's
        // cancel_tones.rs; mirroring here so bo-mux's sanitizer
        // path doesn't regress on the same bug.
        let safe_hz = if fundamental_hz.is_finite() {
            fundamental_hz.max(1.0)
        } else { 1.0 };
        Self {
            sample_rate: sample_rate as f32,
            fundamental_hz: safe_hz,
            phases,
            amplitudes,
        }
    }

    /// Sum of all harmonics at the current phase, returned in roughly
    /// [-1.0, 1.0]. The factor 0.4 below normalizes the sum so we don't
    /// clip when mixed with the masker.
    pub fn next_sample(&mut self) -> f32 {
        let mut sum = 0.0_f32;
        for (i, (phase, amp)) in self.phases.iter_mut().zip(self.amplitudes.iter()).enumerate() {
            let n = (i + 1) as f32;
            sum += amp * phase.sin();
            *phase += 2.0 * PI * self.fundamental_hz * n / self.sample_rate;
            // Keep phase bounded so f32 precision doesn't drift over hours.
            if *phase > 2.0 * PI {
                *phase -= 2.0 * PI;
            }
        }
        // Normalize: harmonic sum ranges roughly [-2.0, 2.0] before scaling
        // (the harmonic series sum can briefly exceed any single harmonic).
        sum * 0.4
    }

    /// Adjust the fundamental on the fly — used by the UI when the user
    /// sweeps the frequency slider to find a match for their noise.
    pub fn set_frequency(&mut self, hz: f32) {
        // Same NaN defense as the constructor.
        self.fundamental_hz = if hz.is_finite() { hz.max(1.0) } else { 1.0 };
    }

    pub fn frequency(&self) -> f32 { self.fundamental_hz }
}

/// Recommended fundamental frequency to start with for each masker preset.
/// These are observed midpoints of the typical noise source's fundamental;
/// the user can adjust from there with a slider in the UI.
pub fn default_fundamental_for(preset: &str) -> f32 {
    match preset {
        // 4-cyl engine at ~80 km/h cruise → ~80 Hz firing rate.
        "engine" => 80.0,
        // Highway tire roar dominant range — pick a useful mid-band.
        "road"   => 120.0,
        // US mains = 60 Hz; EU is 50, but 60 is common enough as a default.
        "fan"    => 60.0,
        // HVAC blowers typically resonate 40-60 Hz.
        "hvac"   => 50.0,
        // Generic / brown — no obvious fundamental; just default low.
        _        => 80.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generator should produce values bounded in [-1, 1] across many samples.
    #[test]
    fn antitone_stays_in_range() {
        let mut g = AntiToneGen::new(48_000, 80.0, 4, true);
        for _ in 0..100_000 {
            let s = g.next_sample();
            assert!(s.is_finite(), "non-finite");
            assert!(s.abs() < 1.0, "out of range: {}", s);
        }
    }

    /// With the same seed/frequency the output is deterministic — important
    /// so testing the mixing logic is reproducible.
    #[test]
    fn antitone_deterministic() {
        let mut a = AntiToneGen::new(48_000, 60.0, 3, true);
        let mut b = AntiToneGen::new(48_000, 60.0, 3, true);
        for _ in 0..1000 {
            assert_eq!(a.next_sample().to_bits(), b.next_sample().to_bits());
        }
    }

    /// Inverse=true should start 180° out of phase from inverse=false at t=0.
    /// At sample 0 (before any advance), the two generators produce values
    /// that are exact negatives of each other.
    #[test]
    fn inverse_starts_180_out_of_phase() {
        let mut normal  = AntiToneGen::new(48_000, 100.0, 1, false);
        let mut inverse = AntiToneGen::new(48_000, 100.0, 1, true);
        // sin(0) = 0 and sin(PI) = ~0 too (numerical), so check at a later
        // sample where the difference is real. Both progress at the same
        // rate; their phase offset stays at PI.
        for _ in 0..10 {
            let n = normal.next_sample();
            let i = inverse.next_sample();
            // Should be near-negatives (small numeric tolerance from sin(PI)).
            assert!((n + i).abs() < 0.01,
                    "normal {} + inverse {} should ≈ 0", n, i);
        }
    }

    /// The generator should produce energy at the requested fundamental.
    /// Verify by running 1 second of samples through a discrete sine
    /// correlator (Goertzel-style point estimate) at the target freq.
    #[test]
    fn fundamental_has_strong_energy() {
        let sr = 48_000.0_f32;
        let target_hz = 80.0_f32;
        let n_samples = 48_000;
        let mut g = AntiToneGen::new(48_000, target_hz, 1, false);
        // Correlate against a same-frequency reference sine.
        let mut corr = 0.0_f32;
        for i in 0..n_samples {
            let s = g.next_sample();
            let ref_sin = (2.0 * PI * target_hz * (i as f32) / sr).sin();
            corr += s * ref_sin;
        }
        // For a pure sine of amplitude A correlated against a unit-amplitude
        // reference over 1 second, the correlation sum is ~A * sr / 2.
        // Our amplitude here is ~0.4, so expect ~0.4 * 48000 / 2 = 9600.
        let expected = 0.4 * sr / 2.0;
        assert!(corr > expected * 0.7,
                "fundamental energy {} below expected ({:.0})", corr, expected);
    }

    #[test]
    fn defaults_match_common_noise_sources() {
        // Sanity-check the defaults are in the right ballpark.
        assert!(default_fundamental_for("engine") >= 40.0 &&
                default_fundamental_for("engine") <= 200.0);
        assert!(default_fundamental_for("fan") >= 40.0 &&
                default_fundamental_for("fan") <= 80.0);
        assert!(default_fundamental_for("hvac") >= 30.0 &&
                default_fundamental_for("hvac") <= 80.0);
    }

    #[test]
    fn set_frequency_takes_effect() {
        let mut g = AntiToneGen::new(48_000, 100.0, 1, false);
        for _ in 0..100 { g.next_sample(); }
        g.set_frequency(200.0);
        assert_eq!(g.frequency(), 200.0);
        // Verify the output really shifts: run 100 more samples and check
        // that the correlation at 200 Hz is now much stronger than at 100.
        let mut c_200 = 0.0; let mut c_100 = 0.0;
        let sr = 48_000.0;
        for i in 0..4800 {
            let s = g.next_sample();
            let t = (i as f32) / sr;
            c_200 += s * (2.0 * PI * 200.0 * t).sin();
            c_100 += s * (2.0 * PI * 100.0 * t).sin();
        }
        assert!(c_200.abs() > c_100.abs() * 3.0,
                "after retune to 200 Hz: c_200={:.1}, c_100={:.1}", c_200, c_100);
    }

    #[test]
    fn anti_tone_nan_frequency_falls_back_to_1hz() {
        // f32::max(NaN, 1.0) returns NaN in Rust (partial_cmp). A NaN
        // fundamental_hz would propagate into every phase increment
        // and silently kill the anti-tone output. Defense coerces
        // non-finite to 1.0 Hz.
        let mut g = AntiToneGen::new(48_000, f32::NAN, 4, true);
        assert_eq!(g.frequency(), 1.0);
        for _ in 0..200 {
            let s = g.next_sample();
            assert!(s.is_finite(),
                "anti-tone with NaN frequency produced non-finite: {}", s);
        }
        // set_frequency must also defend.
        g.set_frequency(f32::INFINITY);
        assert_eq!(g.frequency(), 1.0);
        g.set_frequency(f32::NEG_INFINITY);
        assert_eq!(g.frequency(), 1.0);
        g.set_frequency(60.0);
        assert_eq!(g.frequency(), 60.0);
    }
}
