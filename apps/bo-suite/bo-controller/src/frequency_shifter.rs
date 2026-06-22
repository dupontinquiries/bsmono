// src/frequency_shifter.rs
#![allow(dead_code)]
//
// Iterative octave-shifting noise transformer.
//
// ── The hypothesis ──
//
// Phase-based cancellation has a hard cliff: when the lock is bad,
// emitting anti-noise at f either amplifies (worst case) or barely
// helps (mediocre case). The fuzz harness measured ~50% phase
// failures with realistic jitter.
//
// But the perceptual cost of a noise depends on more than its
// amplitude — it depends on its PERCEIVED FUNDAMENTAL. A 120 Hz
// engine fundamental sits squarely in the equal-loudness curve's
// most-annoying region (~50-300 Hz where industrial low-frequency
// regulations focus). Move the same energy to 1.5 kHz and it
// becomes a hiss easily masked by music.
//
// Octave-shifting (the user's hypothesis): instead of trying to
// CANCEL f, ADD synthesized content at 2f, 3f, 4f with carefully
// shaped amplitudes that make the harmonic envelope look like a
// signal whose fundamental is at 2f. The brain hears the dominant
// peak at 2f and infers "this engine is twice as high-pitched."
// Apply again: 4f. Apply again: 8f. After ~5 passes a 120 Hz
// engine is psychoacoustically a 4 kHz signal.
//
// ── Why this isn't free energy ──
//
// We're ADDING energy, not removing it. Total acoustic power goes
// up with each pass. The bet: the ear's logarithmic sensitivity
// to higher frequencies + the lower auditory annoyance weighting
// at higher frequencies > the cost of adding the energy.
//
// This experiment quantifies that bet by measuring:
//   - Peak frequency at each pass count (does it actually climb?)
//   - Total energy (how much extra acoustic power are we creating?)
//   - Engine-timbre score (the audio_features heuristic that
//     stands in for "perceptually engine-like")
//
// ── Why no phase lock ──
//
// The shifter emits content at HARMONICS of the detected fundamental,
// not at the fundamental itself. So we're not trying to cancel the
// fundamental — we're augmenting harmonics. The phase of our
// added content doesn't need to match anything; we just need to
// inject enough energy at 2f, 3f, etc. to dominate the brain's
// fundamental estimation.
//
// This decouples the technique from the brittle phase-lock
// machinery the rest of the canceller depends on. Even at -20 dB
// phase-lock quality (catastrophic for cancellation) this approach
// works because it's not cancelling — it's reshaping.

use std::f32::consts::PI;

pub const SAMPLE_RATE: f32 = 48_000.0;

pub struct OctaveShifter {
    sample_rate: f32,
    /// One phase accumulator per harmonic generator. Up to 6
    /// harmonics covers 2f through 7f, enough for the perceptual
    /// fundamental to be implied even after several passes.
    phase_accs: [f32; 6],
    /// EWMA of recent fundamental estimates — smooths pass-to-pass
    /// jitter in the detection.
    smoothed_fundamental: f32,
}

impl OctaveShifter {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            phase_accs: [0.0; 6],
            smoothed_fundamental: 0.0,
        }
    }

    /// Estimate the input's dominant frequency via bin-aligned
    /// Goertzel scan + parabolic interpolation. Same algorithm as
    /// passby_canceller::estimate_fundamental but inlined here so
    /// the shifter doesn't pull in the rest of the canceller.
    fn estimate_fundamental(&self, samples: &[f32], lo: f32, hi: f32) -> f32 {
        if samples.len() < 256 { return 0.0; }
        let n = samples.len() as f32;
        let bin_hz = self.sample_rate / n;
        let mut best_k = 0i32;
        let mut best_mag = 0.0f32;
        let k_lo = (lo / bin_hz).ceil() as i32;
        let k_hi = (hi / bin_hz).floor() as i32;
        for k in k_lo..=k_hi {
            let f = k as f32 * bin_hz;
            let m = goertzel(samples, f, self.sample_rate);
            if m > best_mag { best_mag = m; best_k = k; }
        }
        if best_mag < 1.0 { return 0.0; }
        let f_at = |k: i32| -> f32 {
            goertzel(samples, k as f32 * bin_hz, self.sample_rate)
        };
        let m_minus = if best_k > 0 { f_at(best_k - 1) } else { 0.0 };
        let m_plus = f_at(best_k + 1);
        let denom = m_minus - 2.0 * best_mag + m_plus;
        let delta = if denom.abs() > 1e-9 {
            (0.5 * (m_minus - m_plus) / denom).clamp(-0.5, 0.5)
        } else { 0.0 };
        (best_k as f32 + delta) * bin_hz
    }

    /// Apply ONE octave-shift pass. Returns the SYNTHESIZED HARMONIC
    /// CONTENT to be ADDED to the input. The intended use: caller
    /// does `for i in 0..n { input[i] += shifted[i] }` or processes
    /// them separately and sums at the listener.
    ///
    /// Parameters:
    ///   - `scan_lo` / `scan_hi`: frequency range to search for the
    ///     dominant fundamental. Pass 1: 60-300 Hz. Pass 2: 200-800.
    ///     Caller is expected to widen the range as energy moves up.
    ///   - `gain`: overall amplitude scaling for the synthesized
    ///     content. 0.5 = half-amplitude (the user's "half volume
    ///     double frequency pinning" intuition).
    pub fn one_pass(
        &mut self,
        input: &[f32],
        scan_lo: f32, scan_hi: f32,
        gain: f32,
    ) -> Vec<f32> {
        let n = input.len();
        let mut out = vec![0.0_f32; n];

        let f0 = self.estimate_fundamental(input, scan_lo, scan_hi);
        if f0 <= 0.0 { return out; }
        // Smooth the fundamental across calls. Without smoothing,
        // pass-to-pass jitter in the detector causes the harmonic
        // generators to wander.
        const ALPHA: f32 = 0.3;
        self.smoothed_fundamental = if self.smoothed_fundamental > 0.0 {
            (1.0 - ALPHA) * self.smoothed_fundamental + ALPHA * f0
        } else { f0 };
        let f = self.smoothed_fundamental;

        // Amplitude estimate at f via Goertzel — the rms energy
        // at the fundamental tells us how loud to make the
        // harmonics. The user's "half volume" intuition: emit at
        // A/2 for the second harmonic.
        let amp_f = goertzel(input, f, self.sample_rate) / (n as f32);

        // Generate harmonics 2..7 with a 1/n amplitude rolloff.
        // The amplitude shape `A/(2n)` matches what a real engine
        // emits, so the brain reads it as "engine at 2f, 3f, ..."
        // (i.e., a fundamental at 2f). Custom gain on top of that
        // is the "tuning knob."
        for n_idx in 0..6 {
            let harmonic_num = (n_idx + 2) as f32;  // 2..7
            let harmonic_freq = f * harmonic_num;
            if harmonic_freq >= self.sample_rate * 0.45 { break; }
            let harmonic_amp = amp_f * gain / harmonic_num;
            let dphase = 2.0 * PI * harmonic_freq / self.sample_rate;
            let mut phase = self.phase_accs[n_idx];
            for k in 0..n {
                out[k] += harmonic_amp * phase.sin();
                phase += dphase;
                if phase > 2.0 * PI { phase -= 2.0 * PI; }
            }
            self.phase_accs[n_idx] = phase;
        }
        out
    }

    /// Multi-pass: apply N passes sequentially, with the input to
    /// pass k+1 being the SUM of original_input + everything we
    /// added in passes 0..=k.
    ///
    /// Each pass widens its scan range — pass 0 scans 60-300 Hz
    /// (the real engine fundamental), pass 1 widens to look at
    /// the new harmonic content, etc. Returns the ACCUMULATED
    /// added content (caller sums with original for the full
    /// post-processing signal).
    pub fn multi_pass(
        &mut self,
        input: &[f32],
        n_passes: usize,
        gain: f32,
    ) -> Vec<f32> {
        let n = input.len();
        let mut accumulated_addition = vec![0.0_f32; n];
        let mut current_signal: Vec<f32> = input.to_vec();
        // Each pass scans a higher range. The starting range is the
        // typical engine band; subsequent passes scan an octave up.
        let mut scan_lo = 60.0f32;
        let mut scan_hi = 300.0f32;
        for _ in 0..n_passes {
            let added = self.one_pass(&current_signal, scan_lo, scan_hi, gain);
            // Sum into both the running signal (for the next pass's
            // detection) and the accumulated output.
            for k in 0..n {
                current_signal[k] += added[k];
                accumulated_addition[k] += added[k];
            }
            // Move scan range up — we expect the new "fundamental"
            // to be roughly 2× the previous one.
            scan_lo *= 1.8;
            scan_hi *= 1.8;
            if scan_hi > self.sample_rate * 0.4 { break; }
        }
        accumulated_addition
    }
}

/// Inline Goertzel — same as the canceller's, copied here so this
/// module is independent of the canceller's internals.
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
    (real * real + imag * imag).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_engine(freq: f32, n: usize) -> Vec<f32> {
        // Three-harmonic engine signature: f at amp 1.0, 2f at 0.5,
        // 3f at 0.3 (matches synth_passby's engine model).
        (0..n).map(|i| {
            let t = i as f32 / SAMPLE_RATE;
            (2.0 * PI * freq * t).sin()
                + 0.5 * (2.0 * PI * 2.0 * freq * t).sin()
                + 0.3 * (2.0 * PI * 3.0 * freq * t).sin()
        }).collect()
    }

    /// Measure where the PEAK energy lives across a frequency
    /// range. Returns the frequency in Hz with maximum Goertzel
    /// magnitude.
    fn peak_freq(samples: &[f32], lo: f32, hi: f32, step: f32) -> (f32, f32) {
        let mut best_f = 0.0;
        let mut best_m = 0.0;
        let mut f = lo;
        while f <= hi {
            let m = goertzel(samples, f, SAMPLE_RATE);
            if m > best_m { best_m = m; best_f = f; }
            f += step;
        }
        (best_f, best_m)
    }

    /// Total energy in a frequency band (proxy for "how loud is
    /// the signal in this region").
    fn band_energy(samples: &[f32], lo: f32, hi: f32) -> f32 {
        let mut sum = 0.0;
        let mut f = lo;
        let mut count = 0;
        while f <= hi {
            let m = goertzel(samples, f, SAMPLE_RATE);
            sum += m * m;
            count += 1;
            f += 20.0;
        }
        if count == 0 { 0.0 } else { (sum / count as f32).sqrt() / samples.len() as f32 }
    }

    #[test]
    fn one_pass_creates_harmonic_content() {
        let signal = synth_engine(120.0, 8192);
        let mut shifter = OctaveShifter::new(SAMPLE_RATE);
        let added = shifter.one_pass(&signal, 60.0, 300.0, 0.5);
        // The added signal should have energy at 240, 360, 480 Hz
        // (harmonics 2-4 of 120 Hz).
        let amp_at_240 = goertzel(&added, 240.0, SAMPLE_RATE);
        let amp_at_360 = goertzel(&added, 360.0, SAMPLE_RATE);
        let amp_at_120 = goertzel(&added, 120.0, SAMPLE_RATE);
        assert!(amp_at_240 > amp_at_120 * 2.0,
            "added content at 240 Hz ({}) should be much bigger than at 120 Hz ({})",
            amp_at_240, amp_at_120);
        assert!(amp_at_360 > amp_at_120 * 2.0);
    }

    /// The main experiment the user asked for: do passes 1, 2, 5,
    /// 10 actually shift the perceived fundamental upward? Print
    /// the table — this is characterization, not pass/fail.
    #[test]
    fn iterative_passes_shift_perceived_fundamental() {
        let signal = synth_engine(120.0, 16384);
        let baseline_peak = peak_freq(&signal, 60.0, 5000.0, 20.0);
        eprintln!("\n=== iterative octave-shift experiment ===");
        eprintln!("source: 120 Hz engine, 3-harmonic");
        eprintln!("{:>8} {:>12} {:>14} {:>14} {:>14}",
                  "passes", "peak_freq", "peak_mag",
                  "low_band_E", "high_band_E");
        eprintln!("{:>8} {:>12.1} {:>14.3} {:>14.5} {:>14.5}",
                  "baseline", baseline_peak.0, baseline_peak.1,
                  band_energy(&signal, 60.0, 400.0),
                  band_energy(&signal, 1000.0, 5000.0));

        for &n_passes in &[1usize, 2, 5, 10] {
            let mut shifter = OctaveShifter::new(SAMPLE_RATE);
            let added = shifter.multi_pass(&signal, n_passes, 0.5);
            let mut combined = signal.clone();
            for k in 0..combined.len() { combined[k] += added[k]; }
            let (pf, pm) = peak_freq(&combined, 60.0, 8000.0, 20.0);
            let low = band_energy(&combined, 60.0, 400.0);
            let high = band_energy(&combined, 1000.0, 5000.0);
            eprintln!("{:>8} {:>12.1} {:>14.3} {:>14.5} {:>14.5}",
                      n_passes, pf, pm, low, high);
        }
        eprintln!("==========================================\n");

        let mut shifter = OctaveShifter::new(SAMPLE_RATE);
        let added = shifter.multi_pass(&signal, 10, 0.5);
        let mut combined = signal.clone();
        for k in 0..combined.len() { combined[k] += added[k]; }
        let high_after = band_energy(&combined, 1000.0, 5000.0);
        let high_before = band_energy(&signal, 1000.0, 5000.0);
        assert!(high_after > high_before,
            "10 passes should leave more high-band energy than baseline");
    }

    /// Gain sweep — at what gain (if any) does the peak frequency
    /// actually shift upward? This is the more nuanced version of
    /// the hypothesis test. If even gain=4 doesn't shift the peak,
    /// the additive-harmonic-shift idea is fundamentally limited.
    #[test]
    fn gain_sweep_to_find_peak_shift_threshold() {
        let signal = synth_engine(120.0, 16384);
        eprintln!("\n=== gain sweep: does peak shift? ===");
        eprintln!("source: 120 Hz engine, 3-harmonic, 5 passes");
        eprintln!("{:>6} {:>12} {:>14} {:>14}",
                  "gain", "peak_freq", "peak_mag", "total_rms");
        for &gain in &[0.0_f32, 0.5, 1.0, 2.0, 4.0, 8.0] {
            let mut shifter = OctaveShifter::new(SAMPLE_RATE);
            let added = shifter.multi_pass(&signal, 5, gain);
            let mut combined = signal.clone();
            for k in 0..combined.len() { combined[k] += added[k]; }
            let (pf, pm) = peak_freq(&combined, 60.0, 8000.0, 20.0);
            let total: f32 = (combined.iter().map(|s| s*s).sum::<f32>()
                / combined.len() as f32).sqrt();
            eprintln!("{:>6.1} {:>12.1} {:>14.3} {:>14.4}",
                      gain, pf, pm, total);
        }
        eprintln!("=====================================\n");
    }

    /// Alternative hypothesis: instead of ADDING harmonics, SUBTRACT
    /// them. The 2nd harmonic is already in the source; emit inverted
    /// anti-noise at 2f. The fundamental stays but the harmonics are
    /// suppressed — making the engine sound more sine-like, which
    /// is psychoacoustically LESS engine-like (engines have rich
    /// harmonic content; a pure tone reads as "synth"/"alarm", not
    /// "engine"). This may be the user's intuition reframed.
    #[test]
    fn subtractive_harmonic_cancellation_vs_additive() {
        let signal = synth_engine(120.0, 16384);
        eprintln!("\n=== additive vs subtractive at harmonics ===");
        let base_low = band_energy(&signal, 60.0, 400.0);
        let base_h2 = goertzel(&signal, 240.0, SAMPLE_RATE) / signal.len() as f32;
        let base_h3 = goertzel(&signal, 360.0, SAMPLE_RATE) / signal.len() as f32;
        eprintln!("baseline:  fund_band={:.4}, h2={:.4}, h3={:.4}",
                  base_low, base_h2, base_h3);

        // Additive (user's hypothesis): add 2f, 3f synthesis.
        let mut additive = signal.clone();
        let mut shifter = OctaveShifter::new(SAMPLE_RATE);
        let added = shifter.one_pass(&signal, 60.0, 300.0, 0.5);
        for k in 0..additive.len() { additive[k] += added[k]; }
        let add_h2 = goertzel(&additive, 240.0, SAMPLE_RATE) / additive.len() as f32;
        let add_h3 = goertzel(&additive, 360.0, SAMPLE_RATE) / additive.len() as f32;
        eprintln!("additive:  fund_band={:.4}, h2={:.4}, h3={:.4}",
                  band_energy(&additive, 60.0, 400.0), add_h2, add_h3);

        // Subtractive: same synthesis but SUBTRACTED. With phase-lock
        // this would cancel the source's harmonics. Without phase
        // lock (random), it averages to ~no effect; we model the
        // ideal-phase case where it does cancel.
        let mut subtractive = signal.clone();
        let mut shifter2 = OctaveShifter::new(SAMPLE_RATE);
        // The synthesizer doesn't phase-lock to source's harmonics
        // automatically — but for THIS experiment we can simulate
        // the ideal case by directly subtracting the known h2, h3
        // sinusoids.
        let n = signal.len();
        for k in 0..n {
            let t = k as f32 / SAMPLE_RATE;
            // Subtract h2 (which was 0.5 amp in source) and h3
            // (0.3 amp). This is the "ideal cancellation at
            // harmonics" case.
            subtractive[k] -= 0.5 * (2.0 * PI * 240.0 * t).sin();
            subtractive[k] -= 0.3 * (2.0 * PI * 360.0 * t).sin();
        }
        let sub_h2 = goertzel(&subtractive, 240.0, SAMPLE_RATE) / n as f32;
        let sub_h3 = goertzel(&subtractive, 360.0, SAMPLE_RATE) / n as f32;
        eprintln!("subtract:  fund_band={:.4}, h2={:.4}, h3={:.4}",
                  band_energy(&subtractive, 60.0, 400.0), sub_h2, sub_h3);

        // Engine-likeness: a signal with strong harmonics 4-12 is
        // "engine-y"; a pure sine isn't. Subtractive should drop
        // the timbre score significantly.
        let base_timbre = crate::audio_features::engine_timbre_score(
            &signal, 120.0, SAMPLE_RATE);
        let add_timbre = crate::audio_features::engine_timbre_score(
            &additive, 120.0, SAMPLE_RATE);
        let sub_timbre = crate::audio_features::engine_timbre_score(
            &subtractive, 120.0, SAMPLE_RATE);
        eprintln!("timbre:    base={:.3}, additive={:.3}, subtract={:.3}",
                  base_timbre, add_timbre, sub_timbre);
        eprintln!("=============================================\n");
    }

    /// Variant: does total energy stay bounded across passes? We
    /// want shifting, not multiplying. If total energy blows up
    /// past 2-3× baseline, we're not "shifting" we're "compounding."
    #[test]
    fn total_energy_remains_bounded_across_passes() {
        let signal = synth_engine(120.0, 8192);
        let baseline_total: f32 = (signal.iter().map(|s| s*s).sum::<f32>()
            / signal.len() as f32).sqrt();
        for &n_passes in &[1, 5, 10] {
            let mut shifter = OctaveShifter::new(SAMPLE_RATE);
            let added = shifter.multi_pass(&signal, n_passes, 0.5);
            let mut combined = signal.clone();
            for k in 0..combined.len() { combined[k] += added[k]; }
            let total: f32 = (combined.iter().map(|s| s*s).sum::<f32>()
                / combined.len() as f32).sqrt();
            let ratio = total / baseline_total;
            // With gain=0.5 and harmonic 1/n rolloff, theoretical
            // max addition per pass is ~A·0.5·(1/2+1/3+1/4+1/5+1/6+1/7)
            // ≈ A·0.8. Over 10 passes accumulating, expect ratio
            // up to maybe 3-4×.
            assert!(ratio < 5.0,
                "{}-pass ratio {:.2} should stay <5×", n_passes, ratio);
        }
    }

    #[test]
    fn empty_input_produces_empty_output() {
        let mut shifter = OctaveShifter::new(SAMPLE_RATE);
        let out = shifter.one_pass(&[], 60.0, 300.0, 0.5);
        assert!(out.is_empty());
    }

    #[test]
    fn no_detected_fundamental_produces_silence() {
        // All-zeros input has no detectable fundamental.
        let signal = vec![0.0_f32; 4096];
        let mut shifter = OctaveShifter::new(SAMPLE_RATE);
        let out = shifter.one_pass(&signal, 60.0, 300.0, 0.5);
        let max_abs = out.iter().map(|v| v.abs()).fold(0.0_f32, f32::max);
        assert!(max_abs < 1e-6, "silent input → silent output");
    }

    #[test]
    fn nan_input_handled_gracefully() {
        let mut signal = synth_engine(120.0, 4096);
        signal[100] = f32::NAN;
        signal[200] = f32::INFINITY;
        let mut shifter = OctaveShifter::new(SAMPLE_RATE);
        let out = shifter.one_pass(&signal, 60.0, 300.0, 0.5);
        for &v in &out {
            assert!(v.is_finite(), "output should remain finite");
        }
    }

    #[test]
    fn gain_zero_produces_no_addition() {
        let signal = synth_engine(120.0, 4096);
        let mut shifter = OctaveShifter::new(SAMPLE_RATE);
        let out = shifter.one_pass(&signal, 60.0, 300.0, 0.0);
        let max_abs = out.iter().map(|v| v.abs()).fold(0.0_f32, f32::max);
        assert!(max_abs < 1e-6, "gain=0 should produce silence");
    }
}
