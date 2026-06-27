//! gen.rs — signal generators: a sine [`Tone`] oscillator and a [`Noise`] source
//! (white + Voss-McCartney pink). Lifted from bo-suite's backup beeper / spectral
//! masker; useful for synthesis (unendlich game audio), masking noise (rusty-board),
//! and as test signals for the rest of this crate. std + f32 math.

use std::f32::consts::PI;

/// A sine oscillator with a settable frequency, advanced one sample at a time.
/// The phase accumulator wraps at 2π so it stays precise over long runs.
#[derive(Clone, Debug)]
pub struct Tone {
    phase: f32,
    freq: f32,
    sr: f32,
}

impl Tone {
    /// New oscillator at sample rate `sr`, silent (0 Hz) until tuned.
    pub fn new(sr: f32) -> Self {
        Self { phase: 0.0, freq: 0.0, sr }
    }
    /// Set the output frequency in Hz (clamped to ≥ 0).
    pub fn set_frequency(&mut self, freq_hz: f32) {
        self.freq = if freq_hz.is_finite() { freq_hz.max(0.0) } else { 0.0 };
    }
    pub fn frequency(&self) -> f32 {
        self.freq
    }
    /// Reset the phase to 0 (frequency unchanged).
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }
    /// Next sample in `[-1, 1]`; advances phase by `2π·f/sr`.
    pub fn next_sample(&mut self) -> f32 {
        if self.sr <= 0.0 {
            return 0.0;
        }
        let s = self.phase.sin();
        self.phase += 2.0 * PI * self.freq / self.sr;
        if self.phase >= 2.0 * PI {
            self.phase -= 2.0 * PI;
        }
        s
    }
    /// Fill `buf` with successive samples.
    pub fn render(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            *s = self.next_sample();
        }
    }
}

/// White + pink noise. White is a fast LCG (Numerical-Recipes constants) mapped to
/// `[-1, 1]`; pink is Voss-McCartney (5 rows, each updating at a halving rate) so
/// the sum approximates 1/f spectral density. Deterministic for a given seed.
#[derive(Clone, Debug)]
pub struct Noise {
    rng: u32,
    pink_rows: [f32; 5],
    pink_counter: u32,
}

impl Noise {
    /// New generator seeded with `seed` (forced non-zero).
    pub fn new(seed: u32) -> Self {
        Self { rng: seed | 1, pink_rows: [0.0; 5], pink_counter: 0 }
    }
    /// Next white-noise sample in `[-1, 1]`.
    pub fn next_white(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223);
        (self.rng as i32 as f32) / (i32::MAX as f32)
    }
    /// Next pink-noise sample in ~`[-1, 1]` (Voss-McCartney, 5 rows, normalized).
    pub fn next_pink(&mut self) -> f32 {
        self.pink_counter = self.pink_counter.wrapping_add(1);
        let trailing = self.pink_counter.trailing_zeros();
        if (trailing as usize) < self.pink_rows.len() {
            self.pink_rows[trailing as usize] = self.next_white();
        }
        let sum: f32 = self.pink_rows.iter().sum();
        (sum / 5.0).clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{estimate_fundamental, goertzel, level::rms};

    const SR: f32 = 48_000.0;

    #[test]
    fn tone_samples_are_bounded_and_periodic() {
        let mut t = Tone::new(SR);
        t.set_frequency(440.0);
        let mut buf = vec![0.0; 4096];
        t.render(&mut buf);
        assert!(buf.iter().all(|&s| (-1.0..=1.0).contains(&s)));
        assert!(rms(&buf) > 0.5, "a full-scale sine has RMS ≈ 0.707");
    }

    #[test]
    fn generated_tone_is_recovered_by_the_analyzers() {
        // the headline: a tone from `gen` is measured by `goertzel` + `pitch`.
        let mut t = Tone::new(SR);
        t.set_frequency(330.0);
        let mut buf = vec![0.0; 8192];
        t.render(&mut buf);
        assert!(goertzel(&buf, 330.0, SR) > goertzel(&buf, 500.0, SR) * 5.0);
        let f = estimate_fundamental(&buf, 100.0, 1000.0, SR);
        assert!((f - 330.0).abs() < 5.0, "pitch of generated 330 Hz tone = {f}");
    }

    #[test]
    fn zero_sample_rate_tone_is_silent() {
        let mut t = Tone::new(0.0);
        t.set_frequency(440.0);
        assert_eq!(t.next_sample(), 0.0);
    }

    #[test]
    fn white_noise_is_bounded_and_deterministic() {
        let mut a = Noise::new(42);
        let mut b = Noise::new(42);
        for _ in 0..1000 {
            let x = a.next_white();
            assert!((-1.0..=1.0).contains(&x));
            assert_eq!(x, b.next_white(), "same seed → same sequence");
        }
        // distinct seeds diverge
        let mut c = Noise::new(7);
        assert!(Noise::new(42).next_white() != c.next_white());
    }

    #[test]
    fn pink_noise_is_finite_and_bounded() {
        let mut p = Noise::new(99);
        for _ in 0..4096 {
            let x = p.next_pink();
            assert!(x.is_finite() && (-1.0..=1.0).contains(&x));
        }
    }
}
