// src/pattern_learner.rs (vendored from bo-mux)
#![allow(dead_code)]
//
// Predictive cancellation against periodic noise. The insight that makes
// this work despite latency:
//
//   - Mic input gets us samples of the ambient noise.
//   - Motor / fan / compressor noise REPEATS with a stable period.
//   - If we detect that period and learn the per-cycle waveform shape,
//     we can PREDICT the next cycle from the previous one.
//   - We play the inverse of the predicted waveform.
//   - Because we're predicting rather than reacting, latency only needs
//     to be sub-period (not sub-sample) — for a 60 Hz hum (16.67 ms
//     period), the system can tolerate ~5-10 ms of capture/playback
//     latency, which PipeWire quantum scheduling reliably provides.
//
// This module is pure DSP — no I/O, no audio thread, fully testable. It
// exposes three operations:
//
//   1. estimate_period: autocorrelation on a window of mic samples to
//      find the dominant repetition rate.
//   2. learn_template: average many period-aligned windows to extract
//      the clean per-cycle waveform (averaging cancels non-periodic
//      content like air conditioning whoosh or speech).
//   3. periodicity_score: how confident we are the signal IS periodic
//      and worth cancelling. Low scores → bail out, just play the
//      masker noise without phase-locked inverse.
//
// HONEST CAVEATS:
//   - Cancellation effectiveness drops as the source noise drifts away
//     from the learned template. Re-learning every ~10s keeps up with
//     gentle drift (RPM changes, fan ramping).
//   - The cancellation is correct AT THE MIC POSITION; room acoustics
//     mean the listener's ears may not see the same null. Closer the
//     mic is to the listener, better the result.
//   - Works only for QUASI-PERIODIC noise. Transients (door slams) and
//     broadband (waterfall) get ignored — the periodicity_score will
//     be low and the learner won't try.

/// Estimate the dominant period in samples using autocorrelation.
/// Searches `period_min..=period_max` (in samples) for the lag with the
/// strongest self-correlation peak. Returns `None` if no peak stands out.
///
/// Typical use at 48 kHz: period_min=240 (200 Hz upper), period_max=2400
/// (20 Hz lower). Returns fractional samples (the parabolic peak refines
/// the integer-lag estimate to sub-sample accuracy).
pub fn estimate_period(samples: &[f32], period_min: usize, period_max: usize) -> Option<f32> {
    if samples.len() < period_max * 2 { return None; }
    let n = samples.len();
    // Pre-subtract mean to remove DC bias.
    let mean: f32 = samples.iter().sum::<f32>() / n as f32;
    let centered: Vec<f32> = samples.iter().map(|&s| s - mean).collect();

    // Compute energy (R[0]) for normalization.
    let r0: f32 = centered.iter().map(|&s| s * s).sum();
    if r0 < 1e-12 { return None; }

    // Compute R[τ] for τ in the search range. Normalize by R[0] so the
    // returned correlations are in [-1, 1].
    let mut best_lag = period_min;
    let mut best_corr = f32::NEG_INFINITY;
    let mut prev = f32::NEG_INFINITY;
    let mut curr = 0.0;
    let mut found_peak = false;
    for lag in period_min..=period_max.min(n - 1) {
        let max_i = n - lag;
        let mut sum = 0.0;
        for i in 0..max_i { sum += centered[i] * centered[i + lag]; }
        let r = sum / r0;

        // Track the local maximum. Looking for a positive peak so we don't
        // catch alternating spurious peaks.
        if r > best_corr {
            best_corr = r;
            best_lag = lag;
        }
        // Track whether we've seen at least one peak-and-descent.
        if r > 0.3 && r < prev && !found_peak {
            found_peak = true;
        }
        let _ = curr; curr = r;
        prev = r;
    }

    // Sanity threshold: require correlation above ~0.3 to call it
    // periodic at all. Below this we're probably looking at noise.
    if best_corr < 0.30 { return None; }

    // Parabolic refinement around the integer peak for sub-sample
    // accuracy. R(τ-1), R(τ), R(τ+1) fit to a parabola; the vertex
    // gives the refined offset.
    let refined = parabolic_refine(&centered, r0, best_lag);
    Some(refined)
}

fn parabolic_refine(centered: &[f32], r0: f32, lag: usize) -> f32 {
    let r_at = |lag: usize| -> f32 {
        if lag == 0 || lag >= centered.len() { return 0.0; }
        let max_i = centered.len() - lag;
        let mut s = 0.0;
        for i in 0..max_i { s += centered[i] * centered[i + lag]; }
        s / r0
    };
    let y1 = r_at(lag.saturating_sub(1));
    let y2 = r_at(lag);
    let y3 = r_at(lag + 1);
    let denom = 2.0 * (2.0 * y2 - y1 - y3);
    if denom.abs() < 1e-9 { return lag as f32; }
    let offset = (y3 - y1) / denom;
    (lag as f32) + offset.clamp(-1.0, 1.0)
}

/// Build the average per-cycle waveform from a long sample buffer. Given
/// a known period in samples, slices the buffer into period-length
/// windows and averages them. Random noise cancels in the average; the
/// repeating component survives.
///
/// Requires at least 3 full periods to produce a useful template; below
/// that the template is just one slice and isn't worth using.
pub fn learn_template(samples: &[f32], period: f32) -> Option<Vec<f32>> {
    if !period.is_finite() || period < 2.0 { return None; }
    let p_int = period.round() as usize;
    if p_int < 2 { return None; }
    let n_periods = samples.len() / p_int;
    if n_periods < 3 { return None; }

    let mut template = vec![0.0_f32; p_int];
    for k in 0..n_periods {
        let base = k * p_int;
        for i in 0..p_int {
            template[i] += samples[base + i];
        }
    }
    for v in template.iter_mut() { *v /= n_periods as f32; }

    // Center around zero so it can be mixed without adding DC.
    let mean: f32 = template.iter().sum::<f32>() / p_int as f32;
    for v in template.iter_mut() { *v -= mean; }

    Some(template)
}

/// Periodicity score: how much of the signal's energy is captured by
/// the learned template. Range [0, 1]. High values (>0.7) mean the
/// noise is well-described by a repeating template and predictive
/// cancellation will work; low values (<0.3) mean the noise is too
/// broadband or unstable, and we should bail out.
pub fn periodicity_score(samples: &[f32], template: &[f32]) -> f32 {
    if template.is_empty() || samples.len() < template.len() { return 0.0; }
    let p = template.len();
    let n_periods = samples.len() / p;
    if n_periods == 0 { return 0.0; }

    // Total signal energy (mean-removed).
    let n = n_periods * p;
    let sig: &[f32] = &samples[..n];
    let mean: f32 = sig.iter().sum::<f32>() / n as f32;
    let sig_energy: f32 = sig.iter().map(|&s| (s - mean).powi(2)).sum();
    if sig_energy < 1e-12 { return 0.0; }

    // Residual energy — what's left after subtracting the template
    // from each period-aligned window.
    let template_mean: f32 = template.iter().sum::<f32>() / p as f32;
    let mut residual_energy = 0.0_f32;
    for k in 0..n_periods {
        for i in 0..p {
            let r = (sig[k * p + i] - mean) - (template[i] - template_mean);
            residual_energy += r * r;
        }
    }
    let captured = (sig_energy - residual_energy).max(0.0);
    (captured / sig_energy).clamp(0.0, 1.0)
}

/// Output the inverse template at a given phase. `phase_samples` is the
/// integer offset into the (single) cycle; wraps around. Used by the
/// predictor to push samples out one at a time, phase-locked to an
/// internal clock that tracks the learned period.
pub fn inverse_template_at(template: &[f32], phase_samples: usize) -> f32 {
    if template.is_empty() { return 0.0; }
    -template[phase_samples % template.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn synth_sine(freq_hz: f32, sr: u32, n: usize, noise_amp: f32, seed: u32) -> Vec<f32> {
        let mut s = Vec::with_capacity(n);
        let mut state = seed.max(1);
        let mut white = || {
            state ^= state << 13; state ^= state >> 17; state ^= state << 5;
            ((state as f32) / (u32::MAX as f32 / 2.0)) - 1.0
        };
        for i in 0..n {
            let t = i as f32 / sr as f32;
            s.push((2.0 * PI * freq_hz * t).sin() + noise_amp * white());
        }
        s
    }

    #[test]
    fn detects_60hz_in_clean_signal() {
        // 60 Hz at 48k = period of 800 samples exactly.
        let sr = 48_000;
        let s = synth_sine(60.0, sr, sr as usize, 0.0, 1);
        let p = estimate_period(&s, 240, 2400).expect("should find period");
        assert!((p - 800.0).abs() < 1.0,
                "expected period ≈800 samples, got {:.3}", p);
    }

    #[test]
    fn detects_100hz_with_moderate_noise() {
        let sr = 48_000;
        // 100 Hz period = 480 samples. Add 30% noise.
        let s = synth_sine(100.0, sr, sr as usize, 0.3, 7);
        let p = estimate_period(&s, 240, 2400).expect("should find period");
        assert!((p - 480.0).abs() < 3.0,
                "expected ≈480, got {:.3}", p);
    }

    #[test]
    fn returns_none_on_pure_noise() {
        let sr = 48_000;
        let s = synth_sine(60.0, sr, sr as usize, 0.0, 1)
            .iter().map(|_| {
                use std::cell::Cell;
                thread_local! { static R: Cell<u32> = const { Cell::new(0xCAFE) }; }
                R.with(|r| {
                    let mut x = r.get();
                    x ^= x << 13; x ^= x >> 17; x ^= x << 5;
                    r.set(x);
                    ((x as f32) / (u32::MAX as f32 / 2.0)) - 1.0
                })
            }).collect::<Vec<f32>>();
        assert!(estimate_period(&s, 240, 2400).is_none(),
                "pure noise should not yield a period");
    }

    #[test]
    fn learn_template_reproduces_sine() {
        let sr = 48_000;
        let s = synth_sine(80.0, sr, sr as usize, 0.1, 3);  // 600 sample period
        let p = estimate_period(&s, 240, 2400).expect("period");
        let t = learn_template(&s, p).expect("template");
        // Template length should match period.
        assert!((t.len() as f32 - p).abs() <= 1.0);
        // Template should be approximately sinusoidal — peak amplitude
        // near 1.0, and roughly zero-mean after centering.
        let max = t.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let min = t.iter().cloned().fold(f32::INFINITY, f32::min);
        assert!(max > 0.7 && min < -0.7, "expected ±1ish, got [{}, {}]", min, max);
        let mean: f32 = t.iter().sum::<f32>() / t.len() as f32;
        assert!(mean.abs() < 0.05, "template should be zero-mean, got {}", mean);
    }

    #[test]
    fn periodicity_score_high_for_clean_signal() {
        let sr = 48_000;
        let s = synth_sine(60.0, sr, sr as usize, 0.05, 11);  // very clean
        let p = estimate_period(&s, 240, 2400).unwrap();
        let t = learn_template(&s, p).unwrap();
        let score = periodicity_score(&s, &t);
        assert!(score > 0.85,
                "clean periodic signal should score high, got {:.3}", score);
    }

    #[test]
    fn periodicity_score_drops_with_noise() {
        let sr = 48_000;
        let clean_score = {
            let s = synth_sine(60.0, sr, sr as usize, 0.05, 11);
            let p = estimate_period(&s, 240, 2400).unwrap();
            let t = learn_template(&s, p).unwrap();
            periodicity_score(&s, &t)
        };
        let noisy_score = {
            let s = synth_sine(60.0, sr, sr as usize, 0.8, 13);
            if let Some(p) = estimate_period(&s, 240, 2400) {
                if let Some(t) = learn_template(&s, p) {
                    periodicity_score(&s, &t)
                } else { 0.0 }
            } else { 0.0 }
        };
        assert!(noisy_score < clean_score,
                "noisy {:.3} should be < clean {:.3}", noisy_score, clean_score);
    }

    #[test]
    fn inverse_template_negates_at_each_phase() {
        let template = vec![1.0, 0.5, -0.2, -0.7];
        for i in 0..template.len() {
            let inv = inverse_template_at(&template, i);
            assert!((inv + template[i]).abs() < 1e-6,
                    "phase {}: inv {} should negate template {}", i, inv, template[i]);
        }
        // Wrap-around
        assert!((inverse_template_at(&template, 5) + 0.5).abs() < 1e-6);
    }

    #[test]
    fn learn_template_needs_minimum_periods() {
        // 1.5 periods worth of samples — should fail.
        let short = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        assert!(learn_template(&short, 4.0).is_none());
    }
}
