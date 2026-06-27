//! window.rs — analysis windows (rectangular, Hann, Hamming).
//!
//! [`crate::goertzel::spectrogram`] uses a rectangular window (fine for band
//! energy), but spectral and voice-feature work wants a taper to reduce leakage
//! before a transform. These generate a window, apply it in place, or give a
//! single coefficient. std + f32 math.

use std::f32::consts::PI;

/// An analysis window shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Rectangular,
    Hann,
    Hamming,
}

impl Window {
    /// The window coefficient at sample `i` of a length-`n` window. Endpoints of
    /// Hann are 0; of Hamming, 0.08. `n <= 1` yields `1.0`.
    pub fn coeff(self, i: usize, n: usize) -> f32 {
        if n <= 1 {
            return 1.0;
        }
        let x = i as f32 / (n - 1) as f32;
        match self {
            Window::Rectangular => 1.0,
            Window::Hann => 0.5 - 0.5 * (2.0 * PI * x).cos(),
            Window::Hamming => 0.54 - 0.46 * (2.0 * PI * x).cos(),
        }
    }

    /// Generate the full window of length `n`.
    pub fn generate(self, n: usize) -> Vec<f32> {
        (0..n).map(|i| self.coeff(i, n)).collect()
    }

    /// Multiply `buf` by the window in place (length taken from `buf`).
    pub fn apply(self, buf: &mut [f32]) {
        let n = buf.len();
        for (i, s) in buf.iter_mut().enumerate() {
            *s *= self.coeff(i, n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangular_is_all_ones() {
        assert_eq!(Window::Rectangular.generate(8), vec![1.0; 8]);
    }

    #[test]
    fn hann_endpoints_zero_center_one() {
        let w = Window::Hann.generate(65); // odd → exact center sample
        assert!(w[0].abs() < 1e-6 && w[64].abs() < 1e-6, "Hann tapers to 0 at the ends");
        assert!((w[32] - 1.0).abs() < 1e-6, "Hann peaks at 1 in the center");
    }

    #[test]
    fn hamming_endpoints_are_small_nonzero() {
        let w = Window::Hamming.generate(64);
        assert!((w[0] - 0.08).abs() < 1e-2, "Hamming endpoint ≈ 0.08");
    }

    #[test]
    fn apply_scales_in_place() {
        let mut buf = vec![1.0; 65];
        Window::Hann.apply(&mut buf);
        assert!(buf[0].abs() < 1e-6 && (buf[32] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn degenerate_lengths_are_safe() {
        assert_eq!(Window::Hann.coeff(0, 1), 1.0);
        assert_eq!(Window::Hann.generate(0), Vec::<f32>::new());
    }
}
