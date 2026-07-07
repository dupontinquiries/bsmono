//! `sim::fog_volume` — atmospheric (render) fog: a density + colour the bin
//! packs into the existing fog/atmosphere shader uniforms.
//!
//! Fog is denser low and in the distance, thicker with humidity and at night.
//! This is the **render** fog — DISTINCT from `sim::perception_fog` (AI vision
//! occlusion), which this module does not touch. No volumetric fog core existed
//! (verified gap).
//!
//! Composition (bin/integration tier, not here): time-of-day feeds
//! `day_night01`, weather feeds `humidity01`; the bin samples [`density_at`] /
//! [`extinction`] / [`fog_color`] and packs them into the atmosphere uniforms.
//! No shader edits.
//!
//! Pure and deterministic: data in, data out. No GPU, IO, clock, or RNG; no
//! `.unwrap()`/`.expect()`. Every input is sanitised so output is finite and in
//! range (density `>= 0`, extinction and colour bounded).

/// Tunable fog parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogParams {
    /// Calm baseline density (per metre of view ray, before modifiers).
    pub base_density: f32,
    /// Height falloff: `density *= exp(-height_falloff * max(0, y - ground))`.
    pub height_falloff: f32,
    /// Humidity `0..1` — scales density up.
    pub humidity01: f32,
    /// Time of day: `0` night .. `1` noon. Night is a touch foggier.
    pub day_night01: f32,
    /// Fog colour at noon.
    pub color_day: [f32; 3],
    /// Fog colour at night.
    pub color_night: [f32; 3],
}

/// Extra density at full humidity (×(1 + this)).
const HUMIDITY_GAIN: f32 = 1.0;
/// Extra density at full night (×(1 + this)).
const NIGHT_GAIN: f32 = 0.35;

impl Default for FogParams {
    /// Calm baseline: thin fog that thickens low, with humidity, and at night.
    /// `base_density = 0.02`/m (≈ 18% of a 10 m ray lost at ground in clear
    /// noon air), e-folding over ~20 m of height.
    fn default() -> Self {
        FogParams {
            base_density: 0.02,
            height_falloff: 0.05, // ~20 m e-fold
            humidity01: 0.5,
            day_night01: 1.0, // noon = least fog
            color_day: [0.72, 0.78, 0.85],   // pale blue-grey
            color_night: [0.05, 0.07, 0.12], // deep blue
        }
    }
}

#[inline]
fn san(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}
#[inline]
fn clamp01(x: f32) -> f32 {
    san(x).max(0.0).min(1.0)
}
#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}

/// Fog density at world height `y` (ground plane at `ground_y`).
///
/// `density = base_density * (1 + HUMIDITY_GAIN*humidity01)
///          * (1 + NIGHT_GAIN*(1-day_night01)) * exp(-height_falloff*height)`
/// where `height = max(0, y - ground_y)` (below ground is capped at the ground
/// value). Always `>= 0` and finite (the exponent is `<= 0`, so `exp ∈ (0,1]`),
/// and non-increasing as `y` rises.
pub fn density_at(p: &FogParams, y: f32, ground_y: f32) -> f32 {
    let base = san(p.base_density).max(0.0);
    let falloff = san(p.height_falloff).max(0.0);
    let humidity = clamp01(p.humidity01);
    let night = 1.0 - clamp01(p.day_night01);

    let height = (san(y) - san(ground_y)).max(0.0);
    let hum_mult = 1.0 + HUMIDITY_GAIN * humidity;
    let night_mult = 1.0 + NIGHT_GAIN * night;

    let d = base * hum_mult * night_mult * (-falloff * height).exp();
    if d.is_finite() { d.max(0.0) } else { 0.0 }
}

/// Fraction of a view ray of length `view_dist` lost to fog of the given
/// `density`: `1 - exp(-density*view_dist)`, in `[0, 1]`. `0` at
/// `view_dist == 0`; increasing in both `density` and `view_dist`. `density`
/// and `view_dist` are floored at 0.
pub fn extinction(_p: &FogParams, density: f32, view_dist: f32) -> f32 {
    let d = san(density).max(0.0);
    let dist = san(view_dist).max(0.0);
    let e = 1.0 - (-(d * dist)).exp();
    e.clamp(0.0, 1.0)
}

/// Fog colour for the current time of day — `lerp(night, day, day_night01)`,
/// so every channel stays within `[color_night, color_day]`.
pub fn fog_color(p: &FogParams) -> [f32; 3] {
    let t = clamp01(p.day_night01);
    [
        lerp(san(p.color_night[0]), san(p.color_day[0]), t),
        lerp(san(p.color_night[1]), san(p.color_day[1]), t),
        lerp(san(p.color_night[2]), san(p.color_day[2]), t),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(humidity: f32, day_night: f32) -> FogParams {
        FogParams { humidity01: humidity, day_night01: day_night, ..FogParams::default() }
    }

    #[test]
    fn density_nonneg_finite_including_extreme_y() {
        let p = FogParams::default();
        for &y in &[-1.0e30f32, -1000.0, 0.0, 5.0, 1000.0, 1.0e30, f32::NAN, f32::INFINITY] {
            let d = density_at(&p, y, 0.0);
            assert!(d.is_finite() && d >= 0.0, "bad density {} at y={}", d, y);
        }
    }

    #[test]
    fn density_monotone_nonincreasing_in_height() {
        let p = FogParams::default();
        let ground = 3.0;
        let mut prev = density_at(&p, -50.0, ground);
        let mut y = -50.0;
        while y <= 200.0 {
            let d = density_at(&p, y, ground);
            assert!(d <= prev + 1e-7, "density rose with height at y={}", y);
            // strictly decreasing above ground (falloff > 0)
            if y > ground + 1.0 {
                let dd = density_at(&p, y + 1.0, ground);
                assert!(dd < d, "not strictly decreasing above ground at y={}", y);
            }
            prev = d;
            y += 1.0;
        }
        // below ground is capped at the ground value
        assert_eq!(density_at(&p, -10.0, ground), density_at(&p, ground, ground));
    }

    #[test]
    fn humidity_and_night_increase_density() {
        let dry_noon = density_at(&with(0.0, 1.0), 5.0, 0.0);
        let humid_noon = density_at(&with(1.0, 1.0), 5.0, 0.0);
        assert!(humid_noon > dry_noon, "humidity should thicken fog");

        let day = density_at(&with(0.5, 1.0), 5.0, 0.0);
        let night = density_at(&with(0.5, 0.0), 5.0, 0.0);
        assert!(night > day, "night should be foggier");
    }

    #[test]
    fn extinction_in_range_monotone_and_zero_at_zero_dist() {
        let p = FogParams::default();
        assert_eq!(extinction(&p, 0.5, 0.0), 0.0);
        assert_eq!(extinction(&p, 0.0, 100.0), 0.0); // no fog -> no loss

        // monotone in view_dist
        let mut prev = 0.0;
        let mut dist = 0.0;
        while dist <= 500.0 {
            let e = extinction(&p, 0.03, dist);
            assert!((0.0..=1.0).contains(&e));
            assert!(e >= prev - 1e-7, "extinction not increasing in dist");
            prev = e;
            dist += 5.0;
        }
        // monotone in density
        let a = extinction(&p, 0.01, 50.0);
        let b = extinction(&p, 0.05, 50.0);
        assert!(b > a, "extinction should rise with density");

        // extremes stay bounded/finite
        for &(d, v) in &[(1.0e30f32, 1.0e30f32), (f32::NAN, 10.0), (-5.0, -5.0)] {
            let e = extinction(&p, d, v);
            assert!(e.is_finite() && (0.0..=1.0).contains(&e));
        }
    }

    #[test]
    fn fog_color_within_endpoints() {
        let p = FogParams::default();
        // endpoints exact
        let day = { let mut q = p; q.day_night01 = 1.0; fog_color(&q) };
        let night = { let mut q = p; q.day_night01 = 0.0; fog_color(&q) };
        assert_eq!(day, p.color_day);
        assert_eq!(night, p.color_night);

        // deterministic seeded sweep (R20): every channel stays in [night, day]
        let mut x: u64 = 0xF06_C0107_u64.wrapping_add(1);
        let mut next = || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u32 };
        for _ in 0..5_000 {
            let mut q = p;
            q.day_night01 = (next() % 1000) as f32 / 999.0 * 1.4 - 0.2; // incl. out of [0,1]
            let c = fog_color(&q);
            for ch in 0..3 {
                let lo = p.color_night[ch].min(p.color_day[ch]) - 1e-6;
                let hi = p.color_night[ch].max(p.color_day[ch]) + 1e-6;
                assert!(c[ch] >= lo && c[ch] <= hi, "channel {} = {} out of endpoints", ch, c[ch]);
            }
        }
    }

    #[test]
    fn deterministic() {
        let p = FogParams::default();
        assert_eq!(density_at(&p, 7.0, 1.0).to_bits(), density_at(&p, 7.0, 1.0).to_bits());
        assert_eq!(extinction(&p, 0.03, 42.0), extinction(&p, 0.03, 42.0));
        assert_eq!(fog_color(&p), fog_color(&p));
    }
}
