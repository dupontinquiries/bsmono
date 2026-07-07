//! `climate_surface` — the integration point for the per-column climate cores.
//!
//! Previous rounds built a family of pure, tested primitives that nothing yet
//! consumes: [`season_phase`](super::season_phase), [`seasonal_temp_offset`](super::seasonal_temp_offset),
//! [`snow_line`](super::snow_line), and [`snow_cover`](super::snow_cover). Each is
//! correct in isolation, but a consumer (render tint, worldgen surface material)
//! wants a *single* answer for a column, not five scattered calls in the right
//! order. This module is that one call: it composes the primitives into a
//! [`SurfaceClimate`] and is the only thing the edges need to wire.
//!
//! Pure & deterministic — no clock, rng, unwrap, or I/O. The wiring sites apply
//! the result; the base climate fields (and thus worldgen) are never mutated
//! here. Off-by-default at every consumer; this module itself is inert.

use super::{season_phase, seasonal_temp_offset, snow_cover, snow_line};

/// Process-wide gate for the altitude snow-cap *render* feature (`UNENDLICH_SNOW`).
/// Read once and cached. Default off → the render path is byte-identical (the
/// renderer writes a 0 snow-line, which the shader treats as "disabled").
pub fn snow_caps_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("UNENDLICH_SNOW").map(|v| v != "0" && !v.is_empty()).unwrap_or(false)
    })
}

/// Default snow-line altitude (world Y) for the render feature. Above this,
/// terrain albedo trends toward snow white (more on up-facing faces). A constant
/// for now; a debug slider can override it later.
pub const DEFAULT_SNOW_LINE: f32 = 96.0;

/// Resolved per-column surface climate for one tick/position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceClimate {
    /// Smooth annual cycle in `[-1, 1]` (+1 peak summer, −1 peak winter).
    pub season_phase: f32,
    /// Base climate temperature plus the seasonal offset, clamped to `[0, 1]`.
    pub effective_temp: f32,
    /// Snow presence in `[0, 1]` — the max of the altitude (snow-line) signal
    /// and the precipitation/slope (snow-cover) signal, so either a cold peak
    /// OR a cold wet flat reads as snowy.
    pub snow: f32,
}

/// Inputs for a single column. Grouped so call sites read clearly and so adding
/// a field later doesn't churn every caller's positional args.
#[derive(Debug, Clone, Copy)]
pub struct ColumnClimate {
    /// Day within the year (any sign; wrapped internally).
    pub day_of_year: f32,
    /// Days per year (>= 1).
    pub year_length: f32,
    /// Local seasonal swing amplitude (0 = no seasons here, e.g. tropics).
    pub seasonal_amplitude: f32,
    /// Base climate temperature in `[0, 1]` (worldgen field sample).
    pub base_temp: f32,
    /// Precipitation / humidity proxy in `[0, 1]`.
    pub precip: f32,
    /// Surface steepness in `[0, 1]` (0 flat, 1 vertical) — steep sheds snow.
    pub slope: f32,
    /// Column altitude in blocks (world Y).
    pub altitude: f32,
    /// Altitude at which the snow line sits for the *neutral* temperature.
    pub snow_line_altitude: f32,
}

/// Compose the climate primitives for one column. Order matters: the seasonal
/// offset shifts the effective temperature *before* the snow signals read it,
/// so winter genuinely lowers the snow line and raises snow cover.
pub fn resolve(c: ColumnClimate) -> SurfaceClimate {
    let phase = season_phase::phase(c.day_of_year, c.year_length);
    let offset = seasonal_temp_offset::temp_offset(phase, c.seasonal_amplitude);
    let effective_temp = (c.base_temp + offset).clamp(0.0, 1.0);

    let by_altitude = snow_line::snow(c.altitude, effective_temp, c.snow_line_altitude);
    let by_cover = snow_cover::cover(effective_temp, c.precip, c.slope);
    let snow = by_altitude.max(by_cover).clamp(0.0, 1.0);

    SurfaceClimate { season_phase: phase, effective_temp, snow }
}

/// Snow coverage at/above which a column's surface shows a snow block.
pub const SNOW_BLOCK_THRESHOLD: f32 = 0.5;

/// Choose the surface block for a column given its resolved climate and the
/// block worldgen would otherwise place. Snowy solid columns get a [`Snow`]
/// surface; liquids and air pass through (snow doesn't lie on water/lava here).
/// Pure — the worldgen surface-material step calls this; it never mutates the
/// climate fields. Inert until worldgen wires it.
///
/// [`Snow`]: super::blocks::BlockId::Snow
pub fn surface_block(climate: &SurfaceClimate, base: super::blocks::BlockId) -> super::blocks::BlockId {
    use super::blocks::BlockId;
    let unsnowable = matches!(
        base,
        BlockId::Air | BlockId::Water | BlockId::Lava | BlockId::Ice | BlockId::Snow
    );
    if climate.snow >= SNOW_BLOCK_THRESHOLD && !unsnowable {
        BlockId::Snow
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ColumnClimate {
        ColumnClimate {
            day_of_year: 0.0,
            year_length: 360.0,
            seasonal_amplitude: 0.15,
            base_temp: 0.5,
            precip: 0.5,
            slope: 0.0,
            altitude: 64.0,
            snow_line_altitude: 120.0,
        }
    }

    #[test]
    fn outputs_are_in_range() {
        for day in [0.0, 90.0, 180.0, 270.0, 359.0, -45.0] {
            let mut c = base();
            c.day_of_year = day;
            let s = resolve(c);
            assert!((-1.0..=1.0).contains(&s.season_phase), "phase {} oob", s.season_phase);
            assert!((0.0..=1.0).contains(&s.effective_temp), "temp {} oob", s.effective_temp);
            assert!((0.0..=1.0).contains(&s.snow), "snow {} oob", s.snow);
        }
    }

    #[test]
    fn winter_is_colder_than_summer() {
        // Day 0 = deep winter, half-year later = peak summer (season_phase model).
        let mut w = base();
        w.day_of_year = 0.0;
        let mut s = base();
        s.day_of_year = 180.0;
        assert!(
            resolve(w).effective_temp < resolve(s).effective_temp,
            "winter should be colder than summer"
        );
    }

    #[test]
    fn high_cold_peak_is_snowier_than_warm_valley() {
        let mut peak = base();
        peak.altitude = 200.0; // well above the snow line
        peak.base_temp = 0.2;  // cold
        let mut valley = base();
        valley.altitude = 30.0;
        valley.base_temp = 0.8; // warm
        assert!(resolve(peak).snow > resolve(valley).snow);
    }

    #[test]
    fn steep_warm_face_holds_no_snow() {
        let mut c = base();
        c.altitude = 40.0;          // below snow line
        c.base_temp = 0.7;          // warm
        c.slope = 1.0;              // vertical — sheds
        c.snow_line_altitude = 500.0;
        assert_eq!(resolve(c).snow, 0.0);
    }

    #[test]
    fn neutral_amplitude_removes_seasonal_swing() {
        // amplitude 0 → effective temp is just base_temp regardless of day.
        let mut a = base();
        a.seasonal_amplitude = 0.0;
        a.day_of_year = 0.0;
        let mut b = a;
        b.day_of_year = 180.0;
        assert_eq!(resolve(a).effective_temp, resolve(b).effective_temp);
        assert_eq!(resolve(a).effective_temp, a.base_temp);
    }

    #[test]
    fn snowy_solid_surface_becomes_snow() {
        use super::super::blocks::BlockId;
        let c = SurfaceClimate { season_phase: -1.0, effective_temp: 0.1, snow: 0.9 };
        assert_eq!(surface_block(&c, BlockId::Grass), BlockId::Snow);
        assert_eq!(surface_block(&c, BlockId::Stone), BlockId::Snow);
    }

    #[test]
    fn warm_surface_unchanged_and_liquids_never_snow() {
        use super::super::blocks::BlockId;
        let warm = SurfaceClimate { season_phase: 1.0, effective_temp: 0.8, snow: 0.0 };
        assert_eq!(surface_block(&warm, BlockId::Grass), BlockId::Grass);
        // even fully snowy, liquids/air are left alone
        let cold = SurfaceClimate { season_phase: -1.0, effective_temp: 0.0, snow: 1.0 };
        assert_eq!(surface_block(&cold, BlockId::Water), BlockId::Water);
        assert_eq!(surface_block(&cold, BlockId::Lava), BlockId::Lava);
        assert_eq!(surface_block(&cold, BlockId::Air), BlockId::Air);
    }
}
