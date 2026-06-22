//! `sim::light_physics` — sunlight + point lights as physics.
//!
//! The sun tracks time of day (plus latitude & season); point/area sources
//! (torches, lava, glowing dragon eyes) fall off with distance and sum
//! additively. This is the pure light model the renderer and gameplay sample —
//! no GPU, no shaders. Pure, deterministic, finite, panic-free (R1/R6/R11);
//! standalone `rustc --test`-able, release-safe.
//!
//! Composes with: `shading_uniforms` (reads [`sun_elevation`] / [`sun_irradiance`]),
//! `fog_volume` (day/night), the renderer (point-light [`accumulate`]), and a
//! dragon's eye glow as a [`PointLight`].

use std::f32::consts::{FRAC_PI_2, TAU};

/// Earth's axial tilt (radians) — the seasonal declination amplitude.
const AXIAL_TILT: f32 = 0.409_1; // ~23.44°

fn frac(x: f32) -> f32 {
    if x.is_nan() {
        0.0
    } else {
        x.rem_euclid(1.0) // wrap into [0,1) so any day/season input is safe
    }
}

fn lat_clamp(latitude_rad: f32) -> f32 {
    if latitude_rad.is_nan() {
        0.0
    } else {
        latitude_rad.max(-FRAC_PI_2).min(FRAC_PI_2)
    }
}

/// Solar declination for a fractional year (`season01` 0.5 = summer solstice).
fn declination(season01: f32) -> f32 {
    AXIAL_TILT * (TAU * (frac(season01) - 0.25)).sin()
}

/// Hour angle (0 at solar noon = `t01` 0.5).
fn hour_angle(t01: f32) -> f32 {
    TAU * (frac(t01) - 0.5)
}

/// Sun elevation angle in radians (`< 0` below horizon). Feeds shading.
pub fn sun_elevation(t01: f32, latitude_rad: f32, season01: f32) -> f32 {
    let lat = lat_clamp(latitude_rad);
    let decl = declination(season01);
    let h = hour_angle(t01);
    let sin_el = lat.sin() * decl.sin() + lat.cos() * decl.cos() * h.cos();
    sin_el.max(-1.0).min(1.0).asin()
}

/// Sun direction as a unit vector (`+y` up, `+x` east, `+z` north) for all
/// inputs. Its `y` component equals `sin(sun_elevation)`.
pub fn sun_direction(t01: f32, latitude_rad: f32, season01: f32) -> [f32; 3] {
    let lat = lat_clamp(latitude_rad);
    let decl = declination(season01);
    let h = hour_angle(t01);
    let el = sun_elevation(t01, latitude_rad, season01);
    // azimuth from the standard solar formula; atan2 is robust at the poles
    let az = (h.sin()).atan2(h.cos() * lat.sin() - decl.tan() * lat.cos());
    let (se, ce) = (el.sin(), el.cos()); // ce >= 0 since el in [-pi/2, pi/2]
    // unit by construction: ce^2*(sin^2+cos^2) + se^2 = ce^2 + se^2 = 1
    [ce * az.sin(), se, ce * az.cos()]
}

/// Top-of-surface irradiance scalar in `[0,1]`: exactly `0` at/below the horizon,
/// rising smoothly (the soft dawn/dusk tail) to a peak at high sun. Higher in
/// summer (the sun climbs higher at noon).
pub fn sun_irradiance(t01: f32, latitude_rad: f32, season01: f32) -> f32 {
    // sin(elevation): 0 at the horizon, 1 at the zenith, negative below → clamp 0
    sun_elevation(t01, latitude_rad, season01).sin().max(0.0).min(1.0)
}

/// A point/area light source.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PointLight {
    pub pos: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    pub range: f32,
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

/// Contribution of one light at world point `at`: inverse-square-ish
/// (`1/(1+d²)`, so finite `=intensity·color` at `d=0`) with a smooth windowed
/// range cutoff that reaches **exactly 0 at and beyond `range`**. Non-negative
/// per channel (color/intensity floored at 0).
pub fn point_contrib(l: &PointLight, at: [f32; 3]) -> [f32; 3] {
    let range = l.range.max(0.0);
    let d2 = dist2(l.pos, at);
    let r2 = range * range;
    if !(d2 < r2) {
        // at or beyond range (also covers range==0 and any NaN) → no light
        return [0.0, 0.0, 0.0];
    }
    let inv_sq = 1.0 / (1.0 + d2); // 1 at d=0, ~1/d² for large d, always finite
    // smooth window: (1 - (d/range)^2)^2 → 1 at center, 0 at range
    let w = 1.0 - d2 / r2;
    let window = (w * w).max(0.0);
    let scale = l.intensity.max(0.0) * inv_sq * window;
    [
        l.color[0].max(0.0) * scale,
        l.color[1].max(0.0) * scale,
        l.color[2].max(0.0) * scale,
    ]
}

/// Per-channel saturation cap so a pile of bright lights stays finite.
const ACCUM_CAP: f64 = 1.0e9;

/// Sum the contributions of many lights at `at`. Accumulates in `f64` so the
/// result is order-independent to `f32` precision, then clamps each channel to a
/// finite cap.
pub fn accumulate(lights: &[PointLight], at: [f32; 3]) -> [f32; 3] {
    let mut acc = [0.0f64, 0.0, 0.0];
    for l in lights {
        let c = point_contrib(l, at);
        for k in 0..3 {
            let v = c[k] as f64;
            if v.is_finite() {
                acc[k] += v;
            }
        }
    }
    [
        acc[0].min(ACCUM_CAP) as f32,
        acc[1].min(ACCUM_CAP) as f32,
        acc[2].min(ACCUM_CAP) as f32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn len(v: [f32; 3]) -> f32 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }

    #[test]
    fn sun_direction_is_unit_over_a_day() {
        let mut t = 0.0f32;
        while t <= 1.0 {
            for &lat in &[-1.2f32, -0.5, 0.0, 0.5, 1.2, FRAC_PI_2] {
                for &season in &[0.0f32, 0.25, 0.5, 0.75] {
                    let d = sun_direction(t, lat, season);
                    assert!(d.iter().all(|c| c.is_finite()), "non-finite dir");
                    assert!((len(d) - 1.0).abs() < 1e-4, "dir not unit: {} at t={}", len(d), t);
                }
            }
            t += 0.02;
        }
        // out-of-range / NaN inputs still produce a unit vector
        assert!((len(sun_direction(f32::NAN, f32::NAN, 9.5)) - 1.0).abs() < 1e-4);
        assert!((len(sun_direction(7.3, 100.0, -3.2)) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn elevation_sign_day_vs_night() {
        // equator, summer: noon up, midnight down
        assert!(sun_elevation(0.5, 0.0, 0.5) > 0.0, "noon should be above horizon");
        assert!(sun_elevation(0.0, 0.0, 0.5) < 0.0, "midnight should be below horizon");
        // sun_direction.y matches sin(elevation)
        let el = sun_elevation(0.3, 0.4, 0.5);
        assert!((sun_direction(0.3, 0.4, 0.5)[1] - el.sin()).abs() < 1e-5);
    }

    #[test]
    fn irradiance_zero_below_horizon_peak_noon_summer_gt_winter() {
        // below horizon ⇒ exactly 0
        assert_eq!(sun_irradiance(0.0, 0.0, 0.5), 0.0, "midnight irradiance must be 0");
        // noon positive and near peak at the equator in summer
        let noon = sun_irradiance(0.5, 0.0, 0.5);
        assert!(noon > 0.9, "equatorial summer noon should be near peak: {}", noon);
        // noon is the daily peak
        assert!(noon >= sun_irradiance(0.3, 0.0, 0.5) && noon >= sun_irradiance(0.7, 0.0, 0.5));
        // summer noon brighter than winter noon at a northern latitude
        let lat = 0.7f32;
        assert!(sun_irradiance(0.5, lat, 0.5) > sun_irradiance(0.5, lat, 0.0), "summer should beat winter");
        // all irradiance in [0,1]
        let mut t = 0.0f32;
        while t <= 1.0 {
            let i = sun_irradiance(t, 0.5, 0.3);
            assert!((0.0..=1.0).contains(&i));
            t += 0.05;
        }
    }

    #[test]
    fn point_contrib_inverse_square_finite_zero_beyond_range() {
        let l = PointLight { pos: [0.0, 0.0, 0.0], color: [1.0, 1.0, 1.0], intensity: 4.0, range: 10.0 };
        let c0 = point_contrib(&l, [0.0, 0.0, 0.0]);
        let c1 = point_contrib(&l, [1.0, 0.0, 0.0]);
        let c2 = point_contrib(&l, [2.0, 0.0, 0.0]);
        // finite at d=0 and equals intensity*color there
        assert!(c0.iter().all(|v| v.is_finite()));
        assert!((c0[0] - 4.0).abs() < 1e-5, "d=0 should be intensity*color: {}", c0[0]);
        // falls with distance (~inverse-square)
        assert!(c2[0] < c1[0] && c1[0] < c0[0], "should fall off with distance");
        // non-negative
        assert!(c1.iter().all(|&v| v >= 0.0));
        // exactly 0 at and beyond range
        assert_eq!(point_contrib(&l, [10.0, 0.0, 0.0]), [0.0, 0.0, 0.0], "at range");
        assert_eq!(point_contrib(&l, [50.0, 0.0, 0.0]), [0.0, 0.0, 0.0], "beyond range");
        // negative color/intensity floored to non-negative
        let bad = PointLight { pos: [0.0; 3], color: [-1.0, -1.0, -1.0], intensity: -5.0, range: 5.0 };
        assert_eq!(point_contrib(&bad, [1.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn accumulate_finite_and_order_independent() {
        let lights = [
            PointLight { pos: [0.0, 0.0, 0.0], color: [1.0, 0.5, 0.2], intensity: 3.0, range: 20.0 },
            PointLight { pos: [5.0, 0.0, 0.0], color: [0.2, 0.8, 1.0], intensity: 2.0, range: 15.0 },
            PointLight { pos: [0.0, 3.0, 1.0], color: [1.0, 1.0, 1.0], intensity: 1.0, range: 8.0 },
            PointLight { pos: [100.0, 0.0, 0.0], color: [1.0, 1.0, 1.0], intensity: 9.0, range: 4.0 }, // out of reach
        ];
        let at = [1.0, 0.5, 0.0];
        let a = accumulate(&lights, at);
        assert!(a.iter().all(|v| v.is_finite()), "accumulate must be finite");
        // permuted order → same result (f64 accumulation makes it order-independent)
        let perm = [lights[3], lights[1], lights[0], lights[2]];
        let b = accumulate(&perm, at);
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 1e-4, "order-dependent at channel {}: {} vs {}", k, a[k], b[k]);
        }
        // empty ⇒ zero
        assert_eq!(accumulate(&[], at), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn deterministic() {
        assert_eq!(sun_direction(0.42, 0.6, 0.3), sun_direction(0.42, 0.6, 0.3));
        let l = PointLight { pos: [1.0, 2.0, 3.0], color: [0.4, 0.5, 0.6], intensity: 2.0, range: 9.0 };
        assert_eq!(point_contrib(&l, [2.0, 2.0, 2.0]), point_contrib(&l, [2.0, 2.0, 2.0]));
    }
}
