//! `world::seasonal_weather` — let the season drive the physical weather cell.
//!
//! [`crate::world::climate_surface`] resolves a column's season into an
//! `effective_temp` (and snow signal); [`crate::world::weather_cell`] turns an
//! atmosphere + terrain into clouds / precip / storms / wind. This ties them:
//! the climate supplies temperature and a season-driven convective instability
//! (warm air is more unstable → summer storms; winter is cooler, cloudier, and
//! calmer), the caller supplies the terrain/atmosphere half, and we resolve the
//! cell. Composes the two cores; reimplements neither. Pure and deterministic.

use super::climate_surface::SurfaceClimate;
use super::weather_cell::{self, CellInputs, WeatherCell};

/// The terrain + atmosphere half of a cell's weather inputs — everything the
/// season doesn't decide. (Temperature and instability come from the climate.)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtmoInputs {
    pub windward_moisture: f32,
    pub peak_height: f32,
    pub dryness: f32,
    pub altitude_norm: f32,
    pub pressure_diff: f32,
    pub pressure_distance: f32,
    pub wind_coeff: f32,
}

/// Convective instability implied by the climate's temperature: warm air is more
/// unstable (summer thunderstorms), cold air is calmer. `effective_temp` is the
/// `[0,1]` climate temperature, so instability tracks it directly.
fn instability_from_climate(c: &SurfaceClimate) -> f32 {
    c.effective_temp.clamp(0.0, 1.0)
}

/// Resolve a cell's weather with the season driving temperature + storminess.
/// Cool seasons cloud up and stay calm; warm seasons clear but turn convective.
/// Pure and deterministic; every field stays in range (each core clamps).
pub fn seasonal_cell(climate: &SurfaceClimate, atmo: &AtmoInputs) -> WeatherCell {
    let temp_norm = climate.effective_temp.clamp(0.0, 1.0);
    let inputs = CellInputs {
        windward_moisture: atmo.windward_moisture,
        peak_height: atmo.peak_height,
        dryness: atmo.dryness,
        temp_norm,
        altitude_norm: atmo.altitude_norm,
        instability: instability_from_climate(climate),
        pressure_diff: atmo.pressure_diff,
        pressure_distance: atmo.pressure_distance,
        wind_coeff: atmo.wind_coeff,
    };
    weather_cell::resolve_cell(&inputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atmo() -> AtmoInputs {
        AtmoInputs {
            windward_moisture: 0.8,
            peak_height: 0.0,
            dryness: 0.0,
            altitude_norm: 0.2,
            pressure_diff: 10.0,
            pressure_distance: 100.0,
            wind_coeff: 2.0,
        }
    }
    fn climate(season_phase: f32, effective_temp: f32) -> SurfaceClimate {
        SurfaceClimate { season_phase, effective_temp, snow: 0.0 }
    }

    #[test]
    fn summer_is_stormier_than_winter() {
        let winter = seasonal_cell(&climate(-1.0, 0.1), &atmo());
        let summer = seasonal_cell(&climate(1.0, 0.9), &atmo());
        assert!(summer.storm > winter.storm, "warm season is more convectively stormy");
    }

    #[test]
    fn winter_is_cloudier_than_summer() {
        // cloud_coverage favours cool air, so the cold season is overcast.
        let winter = seasonal_cell(&climate(-1.0, 0.1), &atmo());
        let summer = seasonal_cell(&climate(1.0, 0.9), &atmo());
        assert!(winter.cloud > summer.cloud, "cool season holds more cloud");
    }

    #[test]
    fn rain_shadow_still_applies_under_season() {
        let mut shadowed = atmo();
        shadowed.peak_height = 3000.0;
        shadowed.dryness = 0.001;
        let open = seasonal_cell(&climate(0.0, 0.5), &atmo());
        let lee = seasonal_cell(&climate(0.0, 0.5), &shadowed);
        assert!(lee.moisture < open.moisture && lee.precip <= open.precip);
    }

    #[test]
    fn deterministic_and_bounded() {
        let c = climate(0.3, 0.6);
        assert_eq!(seasonal_cell(&c, &atmo()), seasonal_cell(&c, &atmo()));
        for &t in &[0.0f32, 0.5, 1.0] {
            let w = seasonal_cell(&climate(0.0, t), &atmo());
            for v in [w.moisture, w.cloud, w.precip, w.storm] {
                assert!((0.0..=1.0).contains(&v) && v.is_finite());
            }
            assert!(w.wind.is_finite() && w.wind >= 0.0);
        }
    }
}
