//! sky_gradient.rs — pure analytic sky colour from sun elevation.
//!
//! A cheap gradient sky: bluer toward the zenith, warmer toward the horizon
//! (especially at sunset), and dark when the sun is below the horizon.
//! Brightness rises monotonically with sun elevation. Pure, deterministic,
//! panic-free (R1/R11/R21). Not a physical model — a tunable look.

use std::f32::consts::FRAC_PI_2;

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Sky colour for a view ray. `sun_elev` and `view_elev` are elevation angles in
/// radians (0 = horizon, π/2 = zenith; `sun_elev < 0` = night). `turbidity`
/// (~1..10) adds warmth/haze near the horizon. Channels are in `[0,1]`.
pub fn sky_color(sun_elev: f32, view_elev: f32, turbidity: f32) -> [f32; 3] {
    // brightness: 0 at/below horizon, ->1 with the sun overhead (monotone in sun_elev)
    let day = sun_elev.sin().max(0.0);

    // view blend 0 = horizon, 1 = zenith
    let t_view = (view_elev / FRAC_PI_2).max(0.0).min(1.0);

    // palette
    let zenith = [0.15, 0.35, 0.85]; // deep blue
    let horizon_day = [0.60, 0.70, 0.85]; // pale blue near the horizon at midday
    let warm = [0.95, 0.55, 0.30]; // sunset orange

    // sunset warmth: strongest when the sun is within ~0.4 rad of the horizon,
    // boosted a little by turbidity.
    let turb = (turbidity / 10.0).max(0.0).min(1.0);
    let sunset = (1.0 - sun_elev.abs() / 0.4).max(0.0).min(1.0);
    let warm_amt = (sunset + 0.1 * turb).min(1.0);

    let horizon_color = mix3(horizon_day, warm, warm_amt);
    let base = mix3(horizon_color, zenith, t_view); // horizon -> zenith

    let mut c = [base[0] * day, base[1] * day, base[2] * day];
    for ch in c.iter_mut() {
        *ch = ch.max(0.0).min(1.0);
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(c: [f32; 3]) -> f32 {
        0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
    }

    #[test]
    fn sun_below_horizon_is_dark() {
        for &se in &[-0.2f32, -0.5, -1.0] {
            let c = sky_color(se, 0.3, 2.0);
            assert!(c.iter().all(|&x| x < 0.02), "night not dark: {:?}", c);
        }
    }

    #[test]
    fn midday_zenith_is_bluish() {
        let c = sky_color(FRAC_PI_2, FRAC_PI_2, 2.0);
        assert!(c[2] > c[0] && c[2] > c[1], "zenith not bluish: {:?}", c);
    }

    #[test]
    fn sunset_horizon_warmer_than_zenith() {
        // "warmer" == more red-vs-blue bias
        let se = 0.15;
        let horizon = sky_color(se, 0.0, 4.0);
        let zenith = sky_color(se, FRAC_PI_2, 4.0);
        let warmth = |c: [f32; 3]| c[0] - c[2];
        assert!(warmth(horizon) > warmth(zenith), "horizon not warmer: {:?} vs {:?}", horizon, zenith);
    }

    #[test]
    fn channels_in_unit_range() {
        let mut seed = 0x5C0F_0001u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..1000 {
            let se = lcg() * 3.0 - 1.5;
            let ve = lcg() * FRAC_PI_2;
            let tb = lcg() * 12.0;
            let c = sky_color(se, ve, tb);
            for x in c {
                assert!((0.0..=1.0).contains(&x), "channel oob {}", x);
            }
        }
    }

    #[test]
    fn brightness_monotone_with_sun_elevation() {
        let mut prev = -1.0f32;
        let mut se = -FRAC_PI_2;
        while se <= FRAC_PI_2 {
            let l = lum(sky_color(se, 0.0, 3.0));
            assert!(l >= prev - 1e-4, "brightness dropped at sun_elev {}", se);
            prev = l;
            se += 0.02;
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(sky_color(0.5, 0.3, 2.5), sky_color(0.5, 0.3, 2.5));
    }
}
