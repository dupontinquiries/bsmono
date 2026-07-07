//! atmosphere_scatter.rs — pure Rayleigh/Mie in-scatter colour.
//!
//! Phase-weighted single-scattering tint for a view ray, given the cosine of the
//! angle between the view and the sun. Rayleigh (wavelength-dependent, blue-
//! heavy, symmetric forward/back) plus forward-peaked Mie haze that grows with
//! turbidity. Extends `sky_gradient`. Pure, deterministic, panic-free
//! (R1/R11/R21). Not radiometrically calibrated — a tunable look.

use std::f32::consts::PI;

/// Mie asymmetry (forward-scattering) parameter.
const MIE_G: f32 = 0.76;

/// In-scatter colour for scattering-angle cosine `cos_theta` (1 = looking toward
/// the sun) and `turbidity` (~1..10, higher = hazier). All channels are `>= 0`.
pub fn scatter(cos_theta: f32, turbidity: f32) -> [f32; 3] {
    let c = cos_theta.max(-1.0).min(1.0);

    // Rayleigh phase: ∝ (1 + cos²θ) — even in θ, so forward/back symmetric.
    let pr = 3.0 / (16.0 * PI) * (1.0 + c * c);

    // Mie phase (Henyey–Greenstein): forward-peaked for g > 0.
    let g = MIE_G;
    let denom = (1.0 + g * g - 2.0 * g * c).max(1e-4).powf(1.5);
    let pm = (1.0 - g * g) / (4.0 * PI * denom);

    // Rayleigh scatters blue strongest (λ⁻⁴), here as a normalized tint.
    let beta_r = [0.18f32, 0.44, 1.0];
    // Mie is roughly neutral; weight grows with turbidity (haze).
    let haze = (turbidity.max(0.0) / 10.0).min(2.0);
    let beta_m = 0.10 * haze;

    [
        pr * beta_r[0] + pm * beta_m,
        pr * beta_r[1] + pm * beta_m,
        pr * beta_r[2] + pm * beta_m,
    ]
}

// ── r334-fog ────────────────────────────────────────────────────────────────
// Night-gated sun in-scatter colour: the phase-weighted [`scatter`] tint, faded
// to black as the sun sets, ready to feed `scatter_inscatter::inscatter_tinted`
// as `tint_rgb`. Keeps the "in-scatter 0 at night" invariant in one place.
// Pure, finite, panic-free.

/// Sun in-scatter colour for view/sun angle cosine `cos_theta` and `turbidity`,
/// multiplied by a smooth day gate from `sun_elev_sin` (`sun_dir.y`). Exactly
/// `[0,0,0]` at/below the horizon; full [`scatter`] tint with the sun well up.
pub fn sun_inscatter_color(cos_theta: f32, turbidity: f32, sun_elev_sin: f32) -> [f32; 3] {
    let day = (sun_elev_sin / 0.1).clamp(0.0, 1.0);
    let t = scatter(cos_theta, turbidity);
    [t[0] * day, t[1] * day, t[2] * day]
}

#[cfg(test)]
mod r334_fog_tests {
    use super::*;

    #[test]
    fn black_at_night() {
        assert_eq!(sun_inscatter_color(1.0, 4.0, 0.0), [0.0, 0.0, 0.0]);
        assert_eq!(sun_inscatter_color(0.2, 4.0, -0.3), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn daytime_matches_scatter() {
        let day = sun_inscatter_color(0.6, 4.0, 1.0);
        let raw = scatter(0.6, 4.0);
        for i in 0..3 {
            assert!((day[i] - raw[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn nonneg_and_ramps_at_dawn() {
        let low = sun_inscatter_color(0.6, 4.0, 0.02);
        let hi = sun_inscatter_color(0.6, 4.0, 0.2);
        assert!(low[2] >= 0.0 && low[2] < hi[2], "dawn should ramp the tint up");
    }

    #[test]
    fn deterministic() {
        assert_eq!(
            sun_inscatter_color(0.5, 3.0, 0.5)[0].to_bits(),
            sun_inscatter_color(0.5, 3.0, 0.5)[0].to_bits()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(c: [f32; 3]) -> f32 {
        c[0] + c[1] + c[2]
    }

    #[test]
    fn forward_scatter_peaks() {
        // looking toward the sun (cos=1) scatters more than to the side (cos=0)
        let fwd = scatter(1.0, 5.0);
        let side = scatter(0.0, 5.0);
        assert!(sum(fwd) > sum(side), "no forward peak: {} !> {}", sum(fwd), sum(side));
    }

    #[test]
    fn channels_non_negative() {
        let mut seed = 0xA705_0001u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..2000 {
            let c = lcg() * 2.0 - 1.0;
            let tb = lcg() * 12.0;
            let s = scatter(c, tb);
            for ch in s {
                assert!(ch >= 0.0, "negative channel {}", ch);
            }
        }
    }

    #[test]
    fn turbidity_raises_haze() {
        // at a side angle, more turbidity => more Mie => brighter
        let lo = scatter(0.3, 1.0);
        let hi = scatter(0.3, 10.0);
        assert!(sum(hi) > sum(lo), "turbidity didn't raise haze: {} !> {}", sum(hi), sum(lo));
    }

    #[test]
    fn rayleigh_term_symmetric() {
        // with no Mie (turbidity 0) the result is pure Rayleigh: even in cosθ
        for &c in &[0.2f32, 0.5, 0.9] {
            let f = scatter(c, 0.0);
            let b = scatter(-c, 0.0);
            for k in 0..3 {
                assert!((f[k] - b[k]).abs() < 1e-6, "Rayleigh not symmetric at c={}", c);
            }
        }
    }

    #[test]
    fn bounded_and_finite() {
        let mut c = -1.0f32;
        while c <= 1.0 {
            for &tb in &[0.0f32, 1.0, 5.0, 10.0, 50.0] {
                let s = scatter(c, tb);
                for ch in s {
                    assert!(ch.is_finite() && ch < 10.0, "unbounded {}", ch);
                }
            }
            c += 0.01;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(scatter(0.42, 3.0), scatter(0.42, 3.0));
    }
}
