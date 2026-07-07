//! blur_kernel.rs — pure blur kernel generation for the menu-background blur.
//!
//! Produces the tap weights + texel offsets the existing `ui_blur.comp` /
//! `ui_downsample.comp` shaders consume for a separable Gaussian blur. Pure math:
//! deterministic, no clocks, no RNG, no rendering types (R1/R11/R21).

/// A 1D separable blur kernel: `weights[i]` is applied at texel offset
/// `offsets[i]`. The kernel is symmetric about the centre tap (offset 0) and its
/// weights sum to 1.
#[derive(Clone, PartialEq, Debug)]
pub struct BlurKernel {
    pub weights: Vec<f32>,
    pub offsets: Vec<f32>,
}

impl BlurKernel {
    /// Variance (spread) of the kernel about its centre. Larger = blurrier.
    pub fn variance(&self) -> f32 {
        let mut v = 0.0f32;
        for (w, o) in self.weights.iter().zip(self.offsets.iter()) {
            v += w * o * o;
        }
        v
    }
}

/// Gaussian kernel of half-width `radius` (full size `2*radius+1`) with a given
/// `sigma`. A non-positive `sigma` falls back to a small positive value so the
/// result is always finite. Weights are normalized to sum to 1.
pub fn gaussian(radius: usize, sigma: f32) -> BlurKernel {
    let sigma = if sigma > 0.0 { sigma } else { 1e-3 };
    let n = 2 * radius + 1;
    let mut weights = Vec::with_capacity(n);
    let mut offsets = Vec::with_capacity(n);
    let denom = 2.0 * sigma * sigma;
    let mut sum = 0.0f32;
    for i in 0..n {
        let x = i as f32 - radius as f32; // -radius ..= radius
        let w = (-(x * x) / denom).exp();
        weights.push(w);
        offsets.push(x);
        sum += w;
    }
    let inv = if sum > 0.0 { 1.0 / sum } else { 1.0 };
    for w in &mut weights {
        *w *= inv;
    }
    BlurKernel { weights, offsets }
}

/// Gaussian kernel with `sigma` derived from `radius` (`sigma = radius/2`,
/// floored to a small positive value). Convenience for "give me a blur this
/// wide" — spread grows with `radius`.
pub fn gaussian_auto(radius: usize) -> BlurKernel {
    let sigma = (radius as f32 / 2.0).max(1e-3);
    gaussian(radius, sigma)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prop_weights_sum_to_one() {
        for radius in 0..=16usize {
            for &sigma in &[0.5f32, 1.0, 2.5, 6.0] {
                let k = gaussian(radius, sigma);
                let sum: f32 = k.weights.iter().sum();
                assert!((sum - 1.0).abs() < 1e-5, "sum={} r={} s={}", sum, radius, sigma);
            }
        }
    }

    #[test]
    fn prop_symmetric_about_centre() {
        for radius in 0..=16usize {
            let k = gaussian_auto(radius);
            let n = k.weights.len();
            assert_eq!(n, 2 * radius + 1);
            for i in 0..n {
                let j = n - 1 - i;
                assert!((k.weights[i] - k.weights[j]).abs() < 1e-6, "weights asym r={}", radius);
                assert!((k.offsets[i] + k.offsets[j]).abs() < 1e-6, "offsets asym r={}", radius);
            }
            assert_eq!(k.offsets[radius], 0.0, "centre offset not 0");
        }
    }

    #[test]
    fn prop_spread_grows_with_radius() {
        let mut prev = gaussian_auto(1).variance();
        for radius in 2..=16usize {
            let v = gaussian_auto(radius).variance();
            assert!(v > prev, "variance did not grow r={} ({} <= {})", radius, v, prev);
            prev = v;
        }
    }

    #[test]
    fn prop_finite_and_nonnegative() {
        for radius in 0..=16usize {
            for &sigma in &[0.0f32, 1.0, 3.0] {
                let k = gaussian(radius, sigma);
                for &w in &k.weights {
                    assert!(w.is_finite() && w >= 0.0, "bad weight r={} s={}", radius, sigma);
                }
                for &o in &k.offsets {
                    assert!(o.is_finite());
                }
            }
        }
    }

    #[test]
    fn radius_zero_is_single_tap() {
        let k = gaussian(0, 1.0);
        assert_eq!(k.weights, vec![1.0]);
        assert_eq!(k.offsets, vec![0.0]);
    }
}
