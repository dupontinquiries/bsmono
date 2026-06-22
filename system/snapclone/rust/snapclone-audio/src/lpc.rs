//! LPC content channel + source-filter vocoder — the piece that makes the
//! parametric stack audible.
//!
//! Analysis fits an all-pole filter to each frame (autocorrelation +
//! Levinson-Durbin) — that filter IS the spectral envelope, the per-frame
//! "which sound is being made" payload, plus a `gain` (the excitation level).
//! Synthesis runs the inverse: excite the all-pole filter `1/A(z)` with an
//! impulse train at the detected pitch (voiced) or white noise (unvoiced), and
//! out comes robotic-but-intelligible speech.
//!
//! Why this completes the stack: the LPC coefficients are the content/timbre
//! channel (and the vector you'd quantise against the shared codebook — "the
//! store"); the pitch/energy/voicing that drive the excitation come from the
//! `prosody` module ("base tones"). Wire those together and you have an
//! end-to-end coder you can hear (see `examples/lpc_roundtrip.rs`). Bitrate is
//! low — order-10 coeffs + gain + pitch + voicing per 20 ms frame is the
//! ~2.4 kbps LPC-vocoder regime, and codebook-quantising the coeff vector drops
//! it further.
//!
//! What's robotic about it: the excitation is a synthetic impulse train / noise,
//! not the real residual, so the voice quality is the classic LPC "buzz." That's
//! expected for a first pass; a better excitation model (or a neural vocoder on
//! these same parameters) is the quality lever. Pre-emphasis would also help and
//! is an easy add. Note: quantising coeffs can move the filter poles outside the
//! unit circle (instability) — synthesis hard-clamps as a guard; production
//! quantisation should use LSP/reflection coeffs, which stay stable.

pub const LPC_ORDER: usize = 10;

/// Fit an order-`LPC_ORDER` LPC filter to one frame.
/// Returns the predictor coefficients `a[1..=p]` (so the prediction is
/// `x̂[n] = Σ coeffs[k]·x[n-1-k]`) and the excitation `gain` (residual RMS, in
/// normalised units). Analysis is Hamming-windowed; the gain is measured on the
/// actual (unwindowed) frame.
pub fn analyze(frame: &[i16], _sample_rate: u32) -> ([f32; LPC_ORDER], f32) {
    let n = frame.len();
    let mut coeffs = [0.0f32; LPC_ORDER];
    if n <= LPC_ORDER {
        return (coeffs, 0.0);
    }
    // normalise + Hamming window for the autocorrelation
    let x: Vec<f64> = frame.iter().map(|&s| s as f64 / 32_768.0).collect();
    let mut w = vec![0.0f64; n];
    for i in 0..n {
        let hamming = 0.54 - 0.46 * (2.0 * std::f64::consts::PI * i as f64 / (n as f64 - 1.0)).cos();
        w[i] = x[i] * hamming;
    }
    // autocorrelation r[0..=p]
    let p = LPC_ORDER;
    let mut r = vec![0.0f64; p + 1];
    for k in 0..=p {
        let mut acc = 0.0;
        for i in k..n {
            acc += w[i] * w[i - k];
        }
        r[k] = acc;
    }
    if r[0] <= 1.0e-9 {
        return (coeffs, 0.0); // silence
    }
    // tiny white-noise floor for conditioning
    r[0] *= 1.0 + 1.0e-6;

    // Levinson-Durbin → predictor coeffs a[1..=p] for x̂[n]=Σ a[j] x[n-j]
    let mut a = vec![0.0f64; p + 1];
    let mut err = r[0];
    for i in 1..=p {
        let mut acc = r[i];
        for j in 1..i {
            acc -= a[j] * r[i - j];
        }
        let k = acc / err;
        let prev = a.clone();
        a[i] = k;
        for j in 1..i {
            a[j] = prev[j] - k * prev[i - j];
        }
        err *= 1.0 - k * k;
        if err <= 0.0 {
            break;
        }
    }
    for j in 1..=p {
        coeffs[j - 1] = a[j] as f32;
    }

    // gain = RMS of the residual on the ACTUAL (unwindowed) frame
    let res = residual(&x.iter().map(|&v| v as f32).collect::<Vec<_>>(), &coeffs);
    let gain = (res.iter().map(|v| v * v).sum::<f32>() / res.len() as f32).sqrt();
    (coeffs, gain)
}

/// Analysis filter `A(z)`: `e[n] = x[n] − Σ coeffs[k]·x[n-1-k]` (zero history).
pub fn residual(x: &[f32], coeffs: &[f32]) -> Vec<f32> {
    let p = coeffs.len();
    let mut e = vec![0.0f32; x.len()];
    for n in 0..x.len() {
        let mut pred = 0.0f32;
        for k in 0..p {
            if n > k {
                pred += coeffs[k] * x[n - 1 - k];
            }
        }
        e[n] = x[n] - pred;
    }
    e
}

/// Synthesis filter `1/A(z)`: exact inverse of `residual` (zero history), so
/// `synth_residual(residual(x)) == x`. Used to prove the filter is correct.
pub fn synth_residual(e: &[f32], coeffs: &[f32]) -> Vec<f32> {
    let p = coeffs.len();
    let mut x = vec![0.0f32; e.len()];
    for n in 0..e.len() {
        let mut pred = 0.0f32;
        for k in 0..p {
            if n > k {
                pred += coeffs[k] * x[n - 1 - k];
            }
        }
        x[n] = e[n] + pred;
    }
    x
}

/// Stateful robotic vocoder: excite `1/A(z)` with a pitch impulse train
/// (voiced) or noise (unvoiced). State carries across frames for continuity.
pub struct LpcSynth {
    state: [f32; LPC_ORDER], // state[k] = y[n-1-k]
    pitch_phase: f32,
    rng: u32,
}

impl Default for LpcSynth {
    fn default() -> Self { Self::new() }
}

impl LpcSynth {
    pub fn new() -> Self {
        Self { state: [0.0; LPC_ORDER], pitch_phase: 0.0, rng: 0x2545_F491 }
    }

    /// Synthesize `n` samples for one frame. `target_rms` is the source frame's
    /// energy (from prosody) — the output is scaled to match it (so level tracks
    /// the input) and soft-saturated through tanh (so it can never hard-clip).
    pub fn synthesize(
        &mut self, coeffs: &[f32], gain: f32, f0_hz: f32, voiced: bool, n: usize,
        sample_rate: u32, target_rms: f32,
    ) -> Vec<i16> {
        let mut raw = vec![0.0f32; n];
        let period = if voiced && f0_hz > 0.0 { sample_rate as f32 / f0_hz } else { 0.0 };
        for r in raw.iter_mut() {
            // excitation (relative shape; absolute level is set by target_rms below)
            let exc = if period > 0.0 {
                self.pitch_phase += 1.0;
                if self.pitch_phase >= period {
                    self.pitch_phase -= period;
                    gain.max(1.0e-4) * period.sqrt()
                } else {
                    0.0
                }
            } else if gain > 0.0 {
                self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let u = (self.rng >> 8) as f32 / 16_777_216.0 * 2.0 - 1.0;
                u * gain * 1.732_05
            } else {
                0.0
            };
            // all-pole filter: y = exc + Σ coeffs[k]·state[k]
            let mut y = exc;
            for k in 0..LPC_ORDER.min(coeffs.len()) {
                y += coeffs[k] * self.state[k];
            }
            if !y.is_finite() {
                y = 0.0; // instability guard
            }
            for k in (1..LPC_ORDER).rev() {
                self.state[k] = self.state[k - 1];
            }
            self.state[0] = y;
            *r = y;
        }
        // match the source energy, then soft-saturate (tanh ∈ (-1,1) → no clip)
        let cur = (raw.iter().map(|v| v * v).sum::<f32>() / n.max(1) as f32).sqrt();
        let scale = if cur > 1.0e-6 { target_rms / cur } else { 0.0 };
        let mut out = vec![0i16; n];
        for (o, &r) in out.iter_mut().zip(raw.iter()) {
            let y = (r * scale).tanh();
            *o = (y * 32_767.0).clamp(-32_768.0, 32_767.0) as i16;
        }
        out
    }

    pub fn reset(&mut self) {
        self.state = [0.0; LPC_ORDER];
        self.pitch_phase = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn filter_is_exact_inverse() {
        // residual → synth must reconstruct the input for ANY coeffs (algebraic
        // inverse) — this gates the filter implementation/sign convention.
        let mut st: u32 = 0xBEEF;
        let x: Vec<f32> = (0..320)
            .map(|_| {
                st = st.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((st >> 16) & 0xffff) as f32 / 32_768.0 - 1.0
            })
            .collect();
        let coeffs = [0.4f32, -0.3, 0.2, -0.1, 0.05, 0.0, 0.0, 0.0, 0.0, 0.0];
        let e = residual(&x, &coeffs);
        let xr = synth_residual(&e, &coeffs);
        let maxerr = x.iter().zip(&xr).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(maxerr < 1.0e-3, "filter not exact inverse, maxerr={maxerr}");
    }

    #[test]
    fn predictor_reduces_energy_on_voiced() {
        // a voiced-ish signal (two formants) should be well-predicted → residual
        // RMS far below the input RMS (confirms Levinson found a real envelope).
        let sr = 16_000.0;
        let frame: Vec<i16> = (0..320)
            .map(|i| {
                let t = i as f32 / sr;
                let v = 0.5 * (2.0 * std::f32::consts::PI * 150.0 * t).sin()
                    + 0.3 * (2.0 * std::f32::consts::PI * 1200.0 * t).sin();
                (v * 20_000.0) as i16
            })
            .collect();
        let xf: Vec<f32> = frame.iter().map(|&s| s as f32 / 32_768.0).collect();
        let (coeffs, gain) = analyze(&frame, 16_000);
        assert!(gain < rms(&xf) * 0.6, "LPC should reduce energy: gain={gain}, in={}", rms(&xf));
        assert!(coeffs.iter().any(|&c| c != 0.0));
    }

    #[test]
    fn synth_is_bounded_and_voiced_makes_sound() {
        // analyze a real-ish frame for stable coeffs, then synth several frames.
        let frame: Vec<i16> = (0..320)
            .map(|i| ((i as f32 / 16_000.0 * 2.0 * std::f32::consts::PI * 200.0).sin() * 15_000.0) as i16)
            .collect();
        let (coeffs, gain) = analyze(&frame, 16_000);
        let mut synth = LpcSynth::new();
        let mut all = Vec::new();
        for _ in 0..10 {
            all.extend(synth.synthesize(&coeffs, gain.max(0.05), 150.0, true, 320, 16_000, 0.2));
        }
        assert!(all.iter().all(|&s| s.is_finite_i16()), "output must be finite");
        assert!(all.iter().any(|&s| s.abs() > 50), "voiced synth should make sound");
    }

    #[test]
    fn synth_silent_without_excitation() {
        let coeffs = [0.3f32, -0.2, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut synth = LpcSynth::new();
        let out = synth.synthesize(&coeffs, 0.0, 0.0, false, 320, 16_000, 0.0);
        assert!(out.iter().all(|&s| s == 0), "zero gain → silence");
    }

    // tiny helper: i16 is always finite, but keep the intent explicit
    trait FiniteI16 { fn is_finite_i16(self) -> bool; }
    impl FiniteI16 for i16 { fn is_finite_i16(self) -> bool { (-32768..=32767).contains(&(self as i32)) } }
}
