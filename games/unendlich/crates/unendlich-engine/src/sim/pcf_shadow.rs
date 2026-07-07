//! `sim::pcf_shadow` — PCF sample kernel and slope-scaled depth bias.
//!
//! Produces the CPU-side data the shadow render pass consumes:
//! - [`pcf_offsets`] — a `kernel × kernel` grid of UV offsets, centred at
//!   `(0,0)`, scaled by `texel` size. Passed to the fragment shader as a
//!   uniform array.
//! - [`slope_scaled_bias`] — depth bias that grows as a surface grazes the
//!   light (`n·l → 0`), preventing shadow acne on steep surfaces while
//!   keeping it bounded at `max_bias`.
//!
//! Both functions are pure, allocation-minimal, and deterministic.
//! No shader files, render wiring, or GPU calls.

// ---------------------------------------------------------------------------
// PCF kernel offsets
// ---------------------------------------------------------------------------

/// Generate a `kernel × kernel` centred grid of shadow-map UV offsets.
///
/// - `kernel` is clamped to `≥ 1`. Odd values (3, 5, 7) are recommended so
///   the grid is exactly centred at `(0, 0)`.
/// - `texel` is the shadow map texel size in UV space (e.g. `1.0 / map_res`).
/// - The returned `Vec` has exactly `kernel² as usize` elements.
/// - Each `[f32; 2]` is `[col_offset, row_offset]` in `[−(k/2)·t, +(k/2)·t]`.
/// - The sum of all offsets is approximately `(0, 0)` (symmetric grid).
/// - Output is deterministic for given `(kernel, texel)`.
pub fn pcf_offsets(kernel: u8, texel: f32) -> Vec<[f32; 2]> {
    let k = kernel.max(1) as i32;
    let half = (k - 1) as f32 / 2.0; // e.g. k=3 → half=1.0, k=1 → half=0.0
    let mut offsets = Vec::with_capacity((k * k) as usize);
    for row in 0..k {
        for col in 0..k {
            let u = (col as f32 - half) * texel;
            let v = (row as f32 - half) * texel;
            offsets.push([u, v]);
        }
    }
    offsets
}

// ---------------------------------------------------------------------------
// Slope-scaled depth bias
// ---------------------------------------------------------------------------

/// Compute a slope-scaled depth bias for shadow mapping.
///
/// `bias = clamp(base + slope × tan(acos(n_dot_l)), 0, max_bias)`
///
/// - `n_dot_l`: cosine of the angle between surface normal and light direction,
///   typically in `[0, 1]`. Values `≤ 0` (back-facing) return `max_bias`.
/// - `base`: constant bias term (> 0 prevents self-shadowing on flat surfaces).
/// - `slope`: scales the tan term; larger values offset steeper slopes more.
/// - `max_bias`: upper clamp; prevents over-biasing (Peter-panning artefact).
///   Also the result when `n_dot_l ≤ 0` (undefined tan).
/// - Always finite; never NaN even at `n_dot_l = 0.0`.
pub fn slope_scaled_bias(n_dot_l: f32, base: f32, slope: f32, max_bias: f32) -> f32 {
    let max_b = max_bias.max(0.0);
    // Guard: n_dot_l ≤ 0 → surface is back-facing or perpendicular; clamp to max.
    // n_dot_l ≥ 1 → cos = 1, acos = 0, tan = 0 → result = base.
    let ndl = n_dot_l.clamp(0.0, 1.0);
    if ndl <= 0.0 {
        return max_b;
    }
    // tan(acos(x)) = sqrt(1 - x²) / x — numerically stable, avoids trig.
    let sin_theta = (1.0 - ndl * ndl).max(0.0).sqrt();
    let tan_theta = sin_theta / ndl; // ndl > 0 guaranteed here
    let bias = base + slope * tan_theta;
    bias.clamp(0.0, max_b)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-5;

    // ---- pcf_offsets -------------------------------------------------------

    #[test]
    fn offset_count_is_k_squared() {
        for k in [1u8, 2, 3, 5, 7, 9] {
            let offsets = pcf_offsets(k, 1.0);
            assert_eq!(offsets.len(), (k as usize) * (k as usize),
                "k={k}: expected {} offsets, got {}", k as usize * k as usize, offsets.len());
        }
    }

    #[test]
    fn k1_returns_single_zero_offset() {
        let offsets = pcf_offsets(1, 0.001);
        assert_eq!(offsets.len(), 1);
        assert!((offsets[0][0]).abs() < EPS, "k=1 offset U should be 0: {}", offsets[0][0]);
        assert!((offsets[0][1]).abs() < EPS, "k=1 offset V should be 0: {}", offsets[0][1]);
    }

    #[test]
    fn offsets_symmetric_sum_near_zero() {
        for k in [3u8, 5, 7] {
            let offsets = pcf_offsets(k, 0.5);
            let sum_u: f32 = offsets.iter().map(|o| o[0]).sum();
            let sum_v: f32 = offsets.iter().map(|o| o[1]).sum();
            assert!(sum_u.abs() < EPS, "k={k} sum U not ≈0: {sum_u}");
            assert!(sum_v.abs() < EPS, "k={k} sum V not ≈0: {sum_v}");
        }
    }

    #[test]
    fn offsets_within_bounds() {
        let texel = 0.25f32;
        for k in [1u8, 3, 5, 7] {
            let half = (k as f32 - 1.0) / 2.0 * texel + EPS;
            for o in pcf_offsets(k, texel) {
                assert!(o[0].abs() <= half, "k={k} U out of bounds: {}", o[0]);
                assert!(o[1].abs() <= half, "k={k} V out of bounds: {}", o[1]);
            }
        }
    }

    #[test]
    fn offsets_all_finite() {
        for k in [1u8, 3, 9] {
            for o in pcf_offsets(k, 1.0 / 1024.0) {
                assert!(o[0].is_finite() && o[1].is_finite(), "non-finite offset in k={k}");
            }
        }
    }

    #[test]
    fn kernel_zero_clamped_to_one() {
        // kernel=0 should clamp to 1, giving a single (0,0) offset.
        let offsets = pcf_offsets(0, 1.0);
        assert_eq!(offsets.len(), 1);
        assert!((offsets[0][0]).abs() < EPS);
        assert!((offsets[0][1]).abs() < EPS);
    }

    #[test]
    fn offsets_deterministic() {
        let a = pcf_offsets(5, 0.125);
        let b = pcf_offsets(5, 0.125);
        assert_eq!(a, b, "pcf_offsets not deterministic");
    }

    // ---- slope_scaled_bias -------------------------------------------------

    #[test]
    fn bias_grows_as_ndl_approaches_zero() {
        let (base, slope, max_b) = (0.001, 0.005, 0.05);
        let mut prev = slope_scaled_bias(1.0, base, slope, max_b);
        let steps = [0.9f32, 0.7, 0.5, 0.3, 0.15, 0.05, 0.01];
        for &ndl in &steps {
            let b = slope_scaled_bias(ndl, base, slope, max_b);
            assert!(b >= prev - EPS, "bias not monotone at n_dot_l={ndl}: {prev} -> {b}");
            prev = b;
        }
    }

    #[test]
    fn bias_clamps_at_max() {
        let max_b = 0.02f32;
        let b = slope_scaled_bias(0.0, 0.001, 0.1, max_b);
        assert!((b - max_b).abs() < EPS, "bias at n_dot_l=0 should equal max_bias: {b}");
    }

    #[test]
    fn bias_at_ndl_one_equals_base() {
        let base = 0.003f32;
        let b = slope_scaled_bias(1.0, base, 0.01, 1.0);
        // tan(acos(1)) = 0 → bias = base
        assert!((b - base).abs() < EPS, "bias at n_dot_l=1 should equal base: {b}");
    }

    #[test]
    fn bias_never_nan_or_inf() {
        for &ndl in &[0.0f32, 1e-6, 0.01, 0.5, 0.99, 1.0, -0.1, 1.5] {
            let b = slope_scaled_bias(ndl, 0.001, 0.005, 0.05);
            assert!(b.is_finite(), "non-finite bias at n_dot_l={ndl}: {b}");
        }
    }

    #[test]
    fn bias_never_negative() {
        for &ndl in &[0.0f32, 0.01, 0.5, 1.0] {
            let b = slope_scaled_bias(ndl, 0.0, 0.0, 0.05);
            assert!(b >= 0.0, "negative bias at n_dot_l={ndl}: {b}");
        }
    }

    #[test]
    fn back_facing_returns_max_bias() {
        let max_b = 0.03f32;
        // n_dot_l ≤ 0 should return max_bias.
        for &ndl in &[-0.5f32, -0.1, 0.0] {
            let b = slope_scaled_bias(ndl, 0.001, 0.01, max_b);
            assert!((b - max_b).abs() < EPS, "n_dot_l={ndl} should give max_bias: {b}");
        }
    }

    #[test]
    fn bias_deterministic() {
        let a = slope_scaled_bias(0.3, 0.002, 0.008, 0.04);
        let b = slope_scaled_bias(0.3, 0.002, 0.008, 0.04);
        assert_eq!(a.to_bits(), b.to_bits(), "bias not deterministic");
    }

    #[test]
    fn bias_clamps_at_max_bias_not_above() {
        let max_b = 0.01f32;
        // Even with a huge slope, result must not exceed max_bias.
        let b = slope_scaled_bias(0.001, 0.005, 1000.0, max_b);
        assert!(b <= max_b + EPS, "bias {b} exceeds max_bias {max_b}");
    }
}
