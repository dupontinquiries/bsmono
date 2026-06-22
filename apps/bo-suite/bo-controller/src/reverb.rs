// src/reverb.rs
#![allow(dead_code)]
//
// Synthetic reverb for test-signal augmentation.
//
// ── Why this exists ──
//
// All the prior experiments synthesized engines in an anechoic room.
// In practice, a passing car's sound travels through a complex
// acoustic path before reaching the bedroom window:
//
//   * Open yard / driveway / street geometry → sparse early
//     reflections from buildings and ground
//   * Open window / louvers → diffraction and partial absorption
//   * Bedroom walls / ceiling / floor → dense diffuse tail
//
// Total RT60 (time for reverb energy to decay by 60 dB) in a
// typical urban bedroom with windows open is somewhere around
// 300-800 ms. Open spaces add more.
//
// Reverb interacts with cancellation in several non-obvious ways:
//
//   * The mic sees the direct path PLUS the reflections, but the
//     anti-noise speaker emits only one signal — the reverb of
//     the anti-noise has to TRAVEL THROUGH THE SAME ROOM, which
//     adds another acoustic chain to the canceller's residual.
//   * Phase coherence of the source's harmonics is DESTROYED by
//     dense diffuse tails (each reflection adds a randomly-
//     dephased copy). So phase-lock effectiveness should drop in
//     reverberant environments. The per-harmonic mode might
//     actually win here.
//   * The TIMBRE of the source changes with reverb — strong room
//     modes amplify some harmonics and cut others. Saturation
//     matching tuned in an anechoic test set might be wrong for
//     reverberant conditions.
//
// This module synthesizes a configurable reverb. The
// experiment-running test code applies it to the signal BEFORE
// the canceller sees it, so we can re-run the same fuzz harness
// with reverb on and off.
//
// ── Architecture ──
//
// Schroeder-style: tapped delay line with sparse reflection times
// at randomized intervals, each with decaying amplitude. The
// "sparse reflections" model captures the early-reflection part
// of a real room's IR (which dominates the perceptual impression)
// without needing FFT-based convolution for a dense diffuse tail.
//
// 12 reflections by default; their delays are uniformly random
// over the first half of RT60; their amplitudes decay
// exponentially. Polarities alternate between reflections to
// avoid an unphysical "all-positive comb" frequency response.

use std::collections::VecDeque;

pub struct Reverb {
    /// Per-reflection delay in samples.
    delays_samples: Vec<usize>,
    /// Per-reflection signed amplitude (alternating sign for
    /// frequency-spectrum smoothness).
    amplitudes: Vec<f32>,
    /// Ring buffer of recent input samples, sized to the longest
    /// reflection delay.
    history: VecDeque<f32>,
    /// Wet/dry mix: 0 = dry (no reverb), 1 = wet (reverb only),
    /// 0.3 = subtle, 0.6 = strong.
    wet_mix: f32,
}

impl Reverb {
    /// Build a reverb with the given decay time (RT60) and number
    /// of reflections. RT60 of 0.3 s ≈ small live room, 0.6 s ≈
    /// medium hall, 1.5 s ≈ church. The seed makes the reflection
    /// pattern reproducible across test runs.
    pub fn new(rt60_secs: f32, n_reflections: usize, sample_rate: f32, seed: u32) -> Self {
        let mut rng = seed.max(1);
        let mut delays = Vec::with_capacity(n_reflections);
        let mut amps = Vec::with_capacity(n_reflections);
        // Cluster reflections in the first half of the RT60 — that's
        // where early reflections dominate perceptually.
        let max_delay_secs = rt60_secs * 0.5;
        for i in 0..n_reflections {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            // Skip the first 5 ms after direct sound — real walls
            // are never that close.
            let min_delay_secs = 0.005;
            let delay_s = min_delay_secs
                + (rng as f32 / u32::MAX as f32) * (max_delay_secs - min_delay_secs);
            let delay_samples = (delay_s * sample_rate) as usize;
            // -60 dB at RT60, scaled per delay.
            let decay = (-6.91 * delay_s / rt60_secs).exp();
            // Per-reflection amplitude. The 0.3 scaling keeps total
            // reverb-tail energy from overwhelming the direct sound.
            let amp = decay * 0.3;
            let signed_amp = if i % 2 == 0 { amp } else { -amp };
            delays.push(delay_samples);
            amps.push(signed_amp);
        }
        let max_delay = *delays.iter().max().unwrap_or(&0);
        Self {
            delays_samples: delays,
            amplitudes: amps,
            history: VecDeque::with_capacity(max_delay + 1),
            wet_mix: 0.4,
        }
    }

    pub fn set_wet_mix(&mut self, wet: f32) {
        self.wet_mix = wet.clamp(0.0, 1.0);
    }

    pub fn wet_mix(&self) -> f32 { self.wet_mix }

    /// Process a buffer of samples, returning a new buffer with
    /// the reverb applied. The reverb's internal history persists
    /// across calls so this works in streaming use too.
    pub fn apply(&mut self, signal: &[f32]) -> Vec<f32> {
        let n = signal.len();
        let mut out = vec![0.0_f32; n];
        let max_delay = *self.delays_samples.iter().max().unwrap_or(&0);
        for (i, &s) in signal.iter().enumerate() {
            // Sanitize non-finite inputs at the boundary so they
            // never enter the history buffer. Same convention as
            // the rest of the audio path.
            let s = if s.is_finite() { s } else { 0.0 };
            // Push current sample into history.
            self.history.push_back(s);
            while self.history.len() > max_delay + 1 {
                self.history.pop_front();
            }
            // Direct path.
            let direct = s;
            // Wet path: sum of reflected copies.
            let mut wet = 0.0_f32;
            let h_len = self.history.len();
            for (&delay, &amp) in self.delays_samples.iter().zip(&self.amplitudes) {
                if h_len > delay {
                    // history[len-1-delay] is the sample `delay`
                    // ago. delay=0 would alias to current sample;
                    // we skipped that range in the constructor.
                    let h_idx = h_len - 1 - delay;
                    wet += self.history[h_idx] * amp;
                }
            }
            // Mix direct + wet by wet_mix.
            out[i] = direct * (1.0 - self.wet_mix) + (direct + wet) * self.wet_mix;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sr() -> f32 { 48_000.0 }

    #[test]
    fn impulse_response_has_decaying_tail() {
        let mut r = Reverb::new(0.3, 8, sr(), 42);
        r.set_wet_mix(1.0);  // pure wet for visibility
        // Impulse input.
        let mut input = vec![0.0_f32; (sr() * 0.5) as usize];
        input[0] = 1.0;
        let output = r.apply(&input);
        // Output should have non-zero samples after the impulse,
        // due to reflections.
        let nonzero_count = output.iter().filter(|&&x| x.abs() > 1e-4).count();
        assert!(nonzero_count > 1,
            "reverb should produce decaying tail, got {} non-zero samples",
            nonzero_count);
        // Energy in the first 100 ms should be greater than energy
        // in the last 100 ms (decaying).
        let first_100ms_energy: f32 = output.iter()
            .take((sr() * 0.1) as usize)
            .map(|x| x * x).sum();
        let last_100ms_energy: f32 = output.iter()
            .rev().take((sr() * 0.1) as usize)
            .map(|x| x * x).sum();
        assert!(first_100ms_energy > last_100ms_energy * 2.0,
            "early energy ({}) should be much greater than late ({})",
            first_100ms_energy, last_100ms_energy);
    }

    #[test]
    fn dry_mix_passes_through_unchanged() {
        let mut r = Reverb::new(0.5, 8, sr(), 1);
        r.set_wet_mix(0.0);
        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        let output = r.apply(&input);
        for (a, b) in input.iter().zip(&output) {
            assert!((a - b).abs() < 1e-5, "dry mix should pass-through");
        }
    }

    #[test]
    fn output_bounded_for_unit_input() {
        let mut r = Reverb::new(0.5, 12, sr(), 99);
        r.set_wet_mix(0.6);
        let input = vec![1.0_f32; 10_000];
        let output = r.apply(&input);
        // With 12 reflections at decreasing amplitude scaled by
        // 0.3, the total wet contribution is < 3.6. Plus direct
        // = 1. So total stays well below 5 for unit input even
        // in worst case constructive interference.
        for &v in &output {
            assert!(v.is_finite(), "output must be finite");
            assert!(v.abs() < 5.0, "output should stay bounded: {}", v);
        }
    }

    #[test]
    fn nan_input_doesnt_propagate() {
        let mut r = Reverb::new(0.3, 4, sr(), 7);
        let mut input = vec![0.1_f32; 2000];
        input[10] = f32::NAN;
        input[500] = f32::INFINITY;
        let output = r.apply(&input);
        // Sanitization at the boundary means EVERY output sample
        // should be finite, including the ones right after the
        // NaN/Inf positions.
        for (i, &v) in output.iter().enumerate() {
            assert!(v.is_finite(), "output[{}] non-finite: {}", i, v);
        }
    }

    #[test]
    fn pure_tone_preserves_fundamental_freq() {
        // Reverb shouldn't shift frequency — it adds reflections at
        // the SAME frequency. Goertzel at 200 Hz should still find
        // strong energy after reverb is applied.
        use std::f32::consts::PI;
        let mut r = Reverb::new(0.4, 12, sr(), 11);
        r.set_wet_mix(0.5);
        let input: Vec<f32> = (0..8192)
            .map(|i| (2.0 * PI * 200.0 * i as f32 / sr()).sin())
            .collect();
        let output = r.apply(&input);
        // Crude Goertzel: just compute correlation with sin/cos at 200.
        let mut re = 0.0_f32;
        let mut im = 0.0_f32;
        for (i, &v) in output.iter().enumerate() {
            let phase = 2.0 * PI * 200.0 * i as f32 / sr();
            re += v * phase.cos();
            im += v * phase.sin();
        }
        let mag = (re * re + im * im).sqrt();
        // For a unit-amp sine, mag should be > N/4 even after reverb
        // (it adds correlated content, not destroys it).
        assert!(mag > output.len() as f32 / 4.0,
            "200 Hz energy should survive reverb; got mag={}", mag);
    }
}
