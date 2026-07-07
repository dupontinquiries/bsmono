//! `sim::shading_uniforms` — per-frame CPU-side shading uniform computation.
//!
//! Computes sun colour/intensity, ambient/sky tint, and tone-map exposure from
//! sun elevation and sky conditions. The bin uploads these values into the
//! existing lighting/fog/sky shader uniforms each frame; no shader files are
//! touched here.
//!
//! # Composes with
//!
//! - `light_physics` (R41 sibling) — provides `sun_elevation` from time-of-day
//! - `fog_volume` — provides fog colour inputs (consumed separately by the bin)
//!
//! # Guarantee summary
//!
//! All output fields are **finite and non-negative**. `exposure` is in `[0.1, 4.0]`.
//! `tonemap` output channels are in `[0, 1)`. All functions are pure and
//! deterministic.

use core::f32::consts::FRAC_PI_2;

// ── Input types ───────────────────────────────────────────────────────────────

/// Per-frame sky conditions fed by weather/biome state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyParams {
    /// Atmospheric turbidity (haze): `0` = crystal clear, `1` = very hazy.
    pub turbidity01: f32,
    /// Overcast fraction: `0` = clear sky, `1` = fully overcast.
    pub overcast01: f32,
}

// ── Output type ───────────────────────────────────────────────────────────────

/// All CPU-computed shading values for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shading {
    /// Linear RGB colour of the sun disc.
    pub sun_color: [f32; 3],
    /// Sun brightness (lux-like scalar, 0 at night).
    pub sun_intensity: f32,
    /// Linear RGB of the ambient fill light.
    pub ambient_color: [f32; 3],
    /// Ambient brightness.
    pub ambient_intensity: f32,
    /// Sky colour at the zenith.
    pub sky_zenith: [f32; 3],
    /// Sky colour at the horizon.
    pub sky_horizon: [f32; 3],
    /// Tone-map exposure multiplier (applied before Reinhard).
    pub exposure: f32,
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Clamp to non-negative finite.
#[inline]
fn nn(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

/// Clamp `v` to `[lo, hi]`.
#[inline]
fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo { lo } else if v > hi { hi } else { v }
}

/// Linear interpolation `a + t*(b-a)`, `t` clamped to `[0,1]`.
#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    let t = clampf(t, 0.0, 1.0);
    a + (b - a) * t
}

/// Component-wise lerp for RGB.
#[inline]
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

/// Smooth-step for a gentler transition near the horizon.
#[inline]
fn smoothstep(t: f32) -> f32 {
    let t = clampf(t, 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ── Sun colour table ──────────────────────────────────────────────────────────

// Reference chromaticities (linear RGB, approximate sRGB primaries):
//   Horizon sunrise/sunset: warm orange-red
//   Low sun (10–20°):       warm yellow-white
//   Noon (90°):             near-white with slight blue

const SUN_HORIZON: [f32; 3] = [1.00, 0.45, 0.15]; // deep warm orange
const SUN_LOW:     [f32; 3] = [1.00, 0.80, 0.55]; // golden hour
const SUN_NOON:    [f32; 3] = [1.00, 0.97, 0.90]; // near-white, slight warm

/// Sun colour at a given normalised elevation (0 = horizon, 1 = zenith).
fn sun_color_at(t: f32) -> [f32; 3] {
    if t < 0.2 {
        lerp3(SUN_HORIZON, SUN_LOW, t / 0.2)
    } else {
        lerp3(SUN_LOW, SUN_NOON, (t - 0.2) / 0.8)
    }
}

// ── Sky colour table ──────────────────────────────────────────────────────────

const SKY_ZENITH_NIGHT:   [f32; 3] = [0.005, 0.008, 0.020];
const SKY_ZENITH_DAWN:    [f32; 3] = [0.10,  0.15,  0.40];
const SKY_ZENITH_DAY:     [f32; 3] = [0.10,  0.28,  0.75];
const SKY_HORIZON_NIGHT:  [f32; 3] = [0.003, 0.005, 0.010];
const SKY_HORIZON_DAWN:   [f32; 3] = [0.60,  0.25,  0.10];
const SKY_HORIZON_DAY:    [f32; 3] = [0.35,  0.58,  0.90];
const SKY_OVERCAST:       [f32; 3] = [0.55,  0.58,  0.62];

// ── Core function ─────────────────────────────────────────────────────────────

/// Compute per-frame shading uniforms from sun elevation (radians) and sky state.
///
/// `sun_elevation`: angle above/below the horizon in radians.
///   - `< 0`            → below horizon (night)
///   - `0`              → at horizon (sunrise/sunset)
///   - `FRAC_PI_2` (≈1.57) → directly overhead (noon)
///
/// All output fields are finite and non-negative. `exposure` ∈ `[0.1, 4.0]`.
pub fn shading(sun_elevation: f32, sky: SkyParams) -> Shading {
    let elev = if sun_elevation.is_finite() { sun_elevation } else { 0.0 };
    let turb = clampf(if sky.turbidity01.is_finite() { sky.turbidity01 } else { 0.0 }, 0.0, 1.0);
    let ovc  = clampf(if sky.overcast01.is_finite() { sky.overcast01 } else { 0.0 }, 0.0, 1.0);

    // t_elev ∈ [0,1]: 0 at/below horizon, 1 at zenith.
    let t_elev = clampf(elev / FRAC_PI_2, 0.0, 1.0);
    // smooth curve so intensity ramps up quickly from the horizon
    let elev_smooth = smoothstep(t_elev);

    // ── Sun intensity ──
    // Base: 0 below horizon → peak ~1 at zenith. Turbidity dims the sun slightly.
    // Overcast significantly reduces direct sun.
    let sun_base = nn(elev) / FRAC_PI_2; // linear 0→1 with elevation
    let sun_intensity_raw = sun_base * (1.0 - 0.3 * turb) * (1.0 - 0.85 * ovc);
    let sun_intensity = nn(sun_intensity_raw);

    // ── Sun colour ──
    let sun_color_clear = sun_color_at(t_elev);
    // Overcast desaturates sun colour toward white.
    let sun_color = lerp3(sun_color_clear, [1.0, 1.0, 1.0], ovc * 0.5);
    let sun_color = [nn(sun_color[0]), nn(sun_color[1]), nn(sun_color[2])];

    // ── Ambient ──
    // Night: dim cool blue; day: sky-tinted fill; overcast: grey fill (stronger).
    let ambient_night = [0.015, 0.018, 0.040];
    let ambient_day   = [0.12,  0.14,  0.18];
    let ambient_ovc   = [0.28,  0.30,  0.32];

    let ambient_clear = lerp3(ambient_night, ambient_day, elev_smooth);
    let ambient_color_raw = lerp3(ambient_clear, ambient_ovc, ovc);
    // Turbidity slightly warms and lifts ambient.
    let ambient_color = [
        nn(ambient_color_raw[0] + 0.03 * turb),
        nn(ambient_color_raw[1] + 0.01 * turb),
        nn(ambient_color_raw[2]),
    ];
    let ambient_intensity = lerp(0.05, 1.0, elev_smooth + 0.4 * ovc).min(1.0).max(0.0);

    // ── Sky zenith / horizon ──
    let t_dawn = clampf(t_elev / 0.15, 0.0, 1.0); // 0→1 over first 15° of elevation
    let t_day  = clampf((t_elev - 0.15) / 0.85, 0.0, 1.0);

    let zenith_raw = if t_elev < 0.0 {
        SKY_ZENITH_NIGHT
    } else if t_elev < 0.15 {
        lerp3(SKY_ZENITH_NIGHT, SKY_ZENITH_DAWN, t_dawn)
    } else {
        lerp3(SKY_ZENITH_DAWN, SKY_ZENITH_DAY, t_day)
    };
    let horizon_raw = if t_elev < 0.0 {
        SKY_HORIZON_NIGHT
    } else if t_elev < 0.15 {
        lerp3(SKY_HORIZON_NIGHT, SKY_HORIZON_DAWN, t_dawn)
    } else {
        lerp3(SKY_HORIZON_DAWN, SKY_HORIZON_DAY, t_day)
    };

    let sky_zenith  = lerp3(zenith_raw,  SKY_OVERCAST, ovc);
    let sky_horizon = lerp3(horizon_raw, SKY_OVERCAST, ovc);
    let sky_zenith  = [nn(sky_zenith[0]),  nn(sky_zenith[1]),  nn(sky_zenith[2])];
    let sky_horizon = [nn(sky_horizon[0]), nn(sky_horizon[1]), nn(sky_horizon[2])];

    // ── Exposure ──
    // Night: low exposure (scene is dark, boost it a little); noon: bright scene.
    // Range: [0.1, 4.0] per spec.
    let exposure_base = lerp(3.5, 1.0, elev_smooth); // night→high exp; noon→moderate
    let exposure = clampf(exposure_base - 0.5 * ovc, 0.1, 4.0);

    Shading {
        sun_color,
        sun_intensity,
        ambient_color,
        ambient_intensity,
        sky_zenith,
        sky_horizon,
        exposure,
    }
}

// ── Tone mapping ─────────────────────────────────────────────────────────────

/// Reinhard-style per-channel tone map: maps linear HDR colour into `[0, 1)`.
///
/// `exposure` pre-multiplies the input before the Reinhard operator so that
/// `tonemap([1,1,1], 1.0)` ≈ `[0.5, 0.5, 0.5]` (Reinhard midpoint).
/// Output channels are in `[0, 1)` for any finite non-negative input.
/// NaN/inf inputs are treated as 0 (safe floor).
pub fn tonemap(linear: [f32; 3], exposure: f32) -> [f32; 3] {
    let e = if exposure.is_finite() && exposure > 0.0 { exposure } else { 1.0 };
    let map = |c: f32| -> f32 {
        let v = if c.is_finite() && c >= 0.0 { c } else { 0.0 };
        let scaled = v * e;
        // Reinhard: x / (1 + x).  Output ∈ [0, 1).
        scaled / (1.0 + scaled)
    };
    [map(linear[0]), map(linear[1]), map(linear[2])]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const NOON: f32 = FRAC_PI_2;
    const HORIZON: f32 = 0.0;
    const NIGHT: f32 = -0.5;
    const SUNRISE: f32 = 0.05; // just above horizon

    fn clear() -> SkyParams { SkyParams { turbidity01: 0.0, overcast01: 0.0 } }
    fn overcast() -> SkyParams { SkyParams { turbidity01: 0.3, overcast01: 1.0 } }

    fn all_finite_nonneg(s: &Shading) -> bool {
        let fields: &[f32] = &[
            s.sun_color[0], s.sun_color[1], s.sun_color[2],
            s.sun_intensity,
            s.ambient_color[0], s.ambient_color[1], s.ambient_color[2],
            s.ambient_intensity,
            s.sky_zenith[0], s.sky_zenith[1], s.sky_zenith[2],
            s.sky_horizon[0], s.sky_horizon[1], s.sky_horizon[2],
            s.exposure,
        ];
        fields.iter().all(|&v| v.is_finite() && v >= 0.0)
    }

    // ── Night: sun_intensity ≈ 0 ──

    #[test]
    fn night_sun_intensity_near_zero() {
        let s = shading(NIGHT, clear());
        assert!(s.sun_intensity < 0.01, "sun_intensity at night = {}", s.sun_intensity);
    }

    #[test]
    fn below_horizon_no_sun() {
        for &elev in &[-0.01f32, -0.1, -1.0, -FRAC_PI_2] {
            let s = shading(elev, clear());
            assert!(s.sun_intensity < 1e-6,
                "sun_intensity should be 0 below horizon at elev={}, got {}", elev, s.sun_intensity);
        }
    }

    // ── Noon brighter than sunrise ──

    #[test]
    fn noon_brighter_than_sunrise() {
        let s_noon    = shading(NOON, clear());
        let s_sunrise = shading(SUNRISE, clear());
        assert!(s_noon.sun_intensity > s_sunrise.sun_intensity,
            "noon={} sunrise={}", s_noon.sun_intensity, s_sunrise.sun_intensity);
    }

    // ── Horizon sun warmer (R > B) than noon ──

    #[test]
    fn horizon_sun_warmer_than_noon() {
        let s_horizon = shading(HORIZON + 0.001, clear());
        let s_noon    = shading(NOON, clear());
        // Warmth = R - B. Horizon should be warmer.
        let warmth_horizon = s_horizon.sun_color[0] - s_horizon.sun_color[2];
        let warmth_noon    = s_noon.sun_color[0]    - s_noon.sun_color[2];
        assert!(warmth_horizon > warmth_noon,
            "horizon warmth={} noon warmth={}", warmth_horizon, warmth_noon);
    }

    #[test]
    fn horizon_sun_red_greater_than_blue() {
        let s = shading(0.01, clear());
        assert!(s.sun_color[0] > s.sun_color[2],
            "R={} B={}", s.sun_color[0], s.sun_color[2]);
    }

    // ── Overcast lowers sun, lifts ambient ──

    #[test]
    fn overcast_lowers_sun_intensity() {
        let s_clear   = shading(NOON, clear());
        let s_overcast = shading(NOON, overcast());
        assert!(s_overcast.sun_intensity < s_clear.sun_intensity,
            "overcast sun={} clear sun={}", s_overcast.sun_intensity, s_clear.sun_intensity);
    }

    #[test]
    fn overcast_lifts_ambient_intensity() {
        let s_clear   = shading(NIGHT, clear());
        let s_overcast = shading(NIGHT, overcast());
        assert!(s_overcast.ambient_intensity >= s_clear.ambient_intensity,
            "overcast ambient={} clear ambient={}", s_overcast.ambient_intensity, s_clear.ambient_intensity);
    }

    // ── All outputs finite and non-negative ──

    #[test]
    fn all_outputs_finite_nonneg_varied() {
        let elevations = [-1.0f32, -0.1, 0.0, 0.01, 0.3, FRAC_PI_2 / 2.0, FRAC_PI_2];
        let skies = [
            SkyParams { turbidity01: 0.0, overcast01: 0.0 },
            SkyParams { turbidity01: 0.5, overcast01: 0.5 },
            SkyParams { turbidity01: 1.0, overcast01: 1.0 },
        ];
        for &e in &elevations {
            for &sky in &skies {
                let s = shading(e, sky);
                assert!(all_finite_nonneg(&s),
                    "non-finite/negative at elev={} turb={} ovc={}: {:?}",
                    e, sky.turbidity01, sky.overcast01, s);
            }
        }
    }

    #[test]
    fn all_outputs_finite_nonneg_boundary() {
        for &(e, t, o) in &[
            (0.0f32, 0.0f32, 0.0f32),
            (FRAC_PI_2, 1.0, 1.0),
            (-FRAC_PI_2, 0.0, 0.0),
        ] {
            let s = shading(e, SkyParams { turbidity01: t, overcast01: o });
            assert!(all_finite_nonneg(&s));
        }
    }

    #[test]
    fn no_panic_on_nan_inf_inputs() {
        let _ = shading(f32::NAN, SkyParams { turbidity01: f32::NAN, overcast01: f32::INFINITY });
        let _ = shading(f32::INFINITY, SkyParams { turbidity01: 0.0, overcast01: 0.0 });
    }

    // ── Exposure in [0.1, 4.0] ──

    #[test]
    fn exposure_in_band() {
        let elevations = [-1.0f32, -0.1, 0.0, 0.3, FRAC_PI_2 / 2.0, FRAC_PI_2];
        let skies = [
            SkyParams { turbidity01: 0.0, overcast01: 0.0 },
            SkyParams { turbidity01: 1.0, overcast01: 1.0 },
        ];
        for &e in &elevations {
            for &sky in &skies {
                let s = shading(e, sky);
                assert!(s.exposure >= 0.1 && s.exposure <= 4.0,
                    "exposure={} out of band at elev={}", s.exposure, e);
            }
        }
    }

    // ── Monotonicity: sun rises with elevation (clear sky) ──

    #[test]
    fn sun_intensity_nondecreasing_with_elevation() {
        let mut prev = 0.0f32;
        let n = 50;
        for i in 0..=n {
            let elev = (i as f32 / n as f32) * FRAC_PI_2;
            let s = shading(elev, clear());
            assert!(s.sun_intensity >= prev - 1e-5,
                "sun_intensity decreased at elev={}: {} → {}", elev, prev, s.sun_intensity);
            prev = s.sun_intensity;
        }
    }

    // ── tonemap: output in [0,1) ──

    #[test]
    fn tonemap_output_in_range() {
        let inputs: &[[f32; 3]] = &[
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [10.0, 5.0, 2.0],
            [100.0, 100.0, 100.0],
            [0.001, 0.001, 0.001],
        ];
        for &lin in inputs {
            for &exp in &[0.1f32, 1.0, 2.0, 4.0] {
                let out = tonemap(lin, exp);
                for (ch, &v) in out.iter().enumerate() {
                    assert!(v >= 0.0 && v < 1.0,
                        "tonemap ch{ch} out of [0,1): lin={:?} exp={exp} out={:?}", lin, out);
                }
            }
        }
    }

    #[test]
    fn tonemap_monotone_in_input() {
        // For equal exposures, higher linear input → higher output (per channel).
        let n = 100;
        for ch in 0..3 {
            let mut prev = 0.0f32;
            for i in 0..=n {
                let v = i as f32 / 10.0;
                let mut lin = [0.0f32; 3];
                lin[ch] = v;
                let out = tonemap(lin, 1.0);
                assert!(out[ch] >= prev - 1e-7,
                    "tonemap ch{ch} not monotone at v={}: {} → {}", v, prev, out[ch]);
                prev = out[ch];
            }
        }
    }

    #[test]
    fn tonemap_black_is_zero() {
        let out = tonemap([0.0, 0.0, 0.0], 1.0);
        for &v in &out { assert_eq!(v, 0.0); }
    }

    #[test]
    fn tonemap_no_panic_on_bad_inputs() {
        let _ = tonemap([f32::NAN, f32::INFINITY, -1.0], f32::NAN);
        let _ = tonemap([0.0; 3], 0.0);
        let _ = tonemap([0.0; 3], -1.0);
    }

    // ── Determinism ──

    #[test]
    fn shading_deterministic() {
        let s1 = shading(0.4, SkyParams { turbidity01: 0.3, overcast01: 0.2 });
        let s2 = shading(0.4, SkyParams { turbidity01: 0.3, overcast01: 0.2 });
        assert_eq!(s1.sun_intensity.to_bits(), s2.sun_intensity.to_bits());
        assert_eq!(s1.exposure.to_bits(), s2.exposure.to_bits());
    }

    #[test]
    fn tonemap_deterministic() {
        let a = tonemap([0.5, 0.3, 1.2], 1.5);
        let b = tonemap([0.5, 0.3, 1.2], 1.5);
        for i in 0..3 { assert_eq!(a[i].to_bits(), b[i].to_bits()); }
    }
}
