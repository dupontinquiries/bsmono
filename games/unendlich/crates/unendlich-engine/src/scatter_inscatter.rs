//! In-scattered light added along a ray: (1 − transmittance)·light — what the
//! medium scatters toward the eye. Pure, deterministic.

pub fn inscatter(transmittance: f32, light: f32) -> f32 {
    let tr = transmittance.clamp(0.0, 1.0);
    (1.0 - tr) * light.max(0.0)
}

// ── r334-fog ────────────────────────────────────────────────────────────────
// Sun-tinted in-scatter: the grey [`inscatter`] amount, gated to zero at night
// and multiplied by a passed-in atmosphere tint (caller supplies
// `atmosphere_scatter::scatter(cos_theta, turbidity)` — passed as a param so
// this file still gates standalone). Pure, finite, panic-free.

/// Smooth night gate from the sun's elevation sine (`sun_dir.y`): 0 when the
/// sun is at/below the horizon, ramping to 1 a little above it. No hard pop at
/// sunrise/sunset.
fn day_gate(sun_elev_sin: f32) -> f32 {
    // Ramp over the first ~0.1 of elevation sine (~6° above horizon).
    (sun_elev_sin / 0.1).clamp(0.0, 1.0)
}

/// Sun-tinted in-scatter colour added along a view ray.
/// `transmittance` and `sun_light` feed the grey [`inscatter`]; `tint_rgb` is
/// the atmosphere in-scatter colour; `sun_elev_sin` night-gates the whole
/// thing; `gain` is `FogParams::sun_inscatter_gain`. All channels `>= 0`, and
/// exactly `[0,0,0]` when the sun is at/below the horizon.
pub fn inscatter_tinted(
    transmittance: f32,
    sun_light: f32,
    tint_rgb: [f32; 3],
    sun_elev_sin: f32,
    gain: f32,
) -> [f32; 3] {
    let amount = inscatter(transmittance, sun_light) * day_gate(sun_elev_sin) * gain.max(0.0);
    [
        amount * tint_rgb[0].max(0.0),
        amount * tint_rgb[1].max(0.0),
        amount * tint_rgb[2].max(0.0),
    ]
}

#[cfg(test)]
mod r334_fog_tests {
    use super::*;

    #[test]
    fn zero_at_and_below_horizon() {
        assert_eq!(inscatter_tinted(0.2, 5.0, [0.3, 0.5, 1.0], 0.0, 1.0), [0.0, 0.0, 0.0]);
        assert_eq!(inscatter_tinted(0.2, 5.0, [0.3, 0.5, 1.0], -0.4, 1.0), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn daytime_scales_with_tint_and_is_nonneg() {
        let c = inscatter_tinted(0.0, 5.0, [0.3, 0.5, 1.0], 1.0, 1.0);
        // transmittance 0 → full grey inscatter (5.0) × tint.
        assert!((c[2] - 5.0 * 1.0).abs() < 1e-4);
        assert!(c[0] >= 0.0 && c[1] >= 0.0 && c[2] >= 0.0);
        assert!(c[2] > c[0], "blue tint dominates here");
    }

    #[test]
    fn gate_ramps_not_pops() {
        let low = inscatter_tinted(0.0, 5.0, [1.0, 1.0, 1.0], 0.02, 1.0)[0];
        let mid = inscatter_tinted(0.0, 5.0, [1.0, 1.0, 1.0], 0.06, 1.0)[0];
        let hi = inscatter_tinted(0.0, 5.0, [1.0, 1.0, 1.0], 0.2, 1.0)[0];
        assert!(low < mid && mid < hi, "dawn gate should ramp");
    }

    #[test]
    fn gain_zero_kills_it() {
        assert_eq!(inscatter_tinted(0.0, 5.0, [1.0, 1.0, 1.0], 1.0, 0.0), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn negative_tint_clamped() {
        let c = inscatter_tinted(0.0, 5.0, [-1.0, 0.5, 1.0], 1.0, 1.0);
        assert_eq!(c[0], 0.0);
    }

    #[test]
    fn deterministic() {
        let a = inscatter_tinted(0.3, 4.0, [0.4, 0.6, 0.9], 0.5, 1.0);
        let b = inscatter_tinted(0.3, 4.0, [0.4, 0.6, 0.9], 0.5, 1.0);
        assert_eq!(a[0].to_bits(), b[0].to_bits());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn full_transmittance_no_inscatter() { assert_eq!(inscatter(1.0, 5.0), 0.0); }
    #[test] fn zero_transmittance_full_light() { assert!((inscatter(0.0, 5.0) - 5.0).abs() < 1e-6); }
    #[test] fn more_light_more() { assert!(inscatter(0.5, 10.0) > inscatter(0.5, 2.0)); }
    #[test] fn bounded_by_light() { assert!(inscatter(0.3, 5.0) <= 5.0); }
    #[test] fn monotone_in_density() { assert!(inscatter(0.2, 5.0) > inscatter(0.8, 5.0)); }
    #[test] fn deterministic() { assert_eq!(inscatter(0.5, 5.0).to_bits(), inscatter(0.5, 5.0).to_bits()); }
}
