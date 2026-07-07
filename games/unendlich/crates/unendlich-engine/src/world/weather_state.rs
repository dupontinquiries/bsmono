//! `world::weather_state` — the capstone that resolves a location's full local
//! weather in one step, tying the layers together so the rest of the engine has
//! a single thing to call:
//!
//! * [`seasonal_weather`](crate::world::seasonal_weather) — season + atmosphere ⇒
//!   the instantaneous cell (cloud / precip / storm / wind / moisture);
//! * [`storm_organization`](crate::world::storm_organization) — vertical shear ⇒
//!   how organized that storm is, and a coarse [`StormMode`];
//! * [`snowpack`](crate::world::snowpack) — the cell's precip integrated into a
//!   snow depth over `dt`.
//!
//! Pure and deterministic: the previous snow depth is the only carried state,
//! passed in and returned inside [`LocalWeather`]. This composes the layers; it
//! adds no new physics of its own.

use super::climate_surface::SurfaceClimate;
use super::seasonal_weather::{self, AtmoInputs};
use super::snowpack;
use super::storm_organization::{self, StormMode};
use super::weather_cell::WeatherCell;

/// The fully-resolved local weather for one step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalWeather {
    /// Instantaneous cell weather (cloud / precip / storm / wind / moisture).
    pub cell: WeatherCell,
    /// Storm organization in `[0,1]` from the deep-layer shear.
    pub organization: f32,
    /// Coarse storm mode.
    pub mode: StormMode,
    /// Snow depth after this step.
    pub snow_depth: f32,
}

/// Everything `weather_state` needs beyond the climate: the atmosphere/terrain
/// (`atmo`), the shear geometry, the °C range that de-normalizes temperature for
/// the snow pack, and the step length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepInputs {
    pub atmo: AtmoInputs,
    /// Depth over which deep-layer shear is measured.
    pub shear_top_altitude: f32,
    /// Shear coefficient (wind strengthening with height).
    pub shear: f32,
    /// °C at climate temp 0.0 and 1.0 — see `snowpack::to_celsius`.
    pub temp_min_c: f32,
    pub temp_max_c: f32,
    /// Step length.
    pub dt: f32,
}

/// Resolve the local weather for one step from the `climate`, the `inputs`, and
/// the `prev_snow_depth` carried from the last step. Pure.
pub fn step(climate: &SurfaceClimate, inputs: &StepInputs, prev_snow_depth: f32) -> LocalWeather {
    let cell = seasonal_weather::seasonal_cell(climate, &inputs.atmo);
    let (organization, mode) =
        storm_organization::organize_cell(&cell, inputs.shear_top_altitude, inputs.shear);
    let snow_depth = snowpack::step_norm(
        prev_snow_depth,
        &cell,
        climate.effective_temp,
        inputs.temp_min_c,
        inputs.temp_max_c,
        inputs.dt,
    );
    LocalWeather { cell, organization, mode, snow_depth }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> StepInputs {
        StepInputs {
            atmo: AtmoInputs {
                windward_moisture: 0.85,
                peak_height: 0.0,
                dryness: 0.0,
                altitude_norm: 0.2,
                pressure_diff: 20.0,
                pressure_distance: 100.0,
                wind_coeff: 2.0,
            },
            shear_top_altitude: 4000.0,
            shear: 1.5,
            temp_min_c: -10.0,
            temp_max_c: 30.0,
            dt: 1.0,
        }
    }
    fn climate(season_phase: f32, effective_temp: f32) -> SurfaceClimate {
        SurfaceClimate { season_phase, effective_temp, snow: 0.0 }
    }

    #[test]
    fn a_cold_wet_step_snows_and_resolves_a_storm() {
        // norm temp 0.1 ⇒ -6 °C ⇒ below freezing ⇒ snow builds.
        let w = step(&climate(-0.8, 0.1), &inputs(), 0.0);
        assert!(w.snow_depth > 0.0, "cold + precipitating ⇒ snow accumulates");
        for v in [w.cell.cloud, w.cell.precip, w.cell.storm, w.organization] {
            assert!((0.0..=1.0).contains(&v) && v.is_finite());
        }
    }

    #[test]
    fn a_warm_step_melts_carried_snow() {
        // norm temp 0.95 ⇒ +28 °C ⇒ melts the pack we carry in.
        let w = step(&climate(0.8, 0.95), &inputs(), 4.0);
        assert!(w.snow_depth < 4.0, "a warm step melts carried snow");
        assert!(w.snow_depth >= 0.0);
    }

    #[test]
    fn shear_drives_storm_mode() {
        let mut calm = inputs();
        calm.shear = 0.02;
        calm.shear_top_altitude = 300.0;
        let sheared = inputs(); // strong deep-layer shear
        let hot = climate(0.9, 0.85); // warm ⇒ unstable ⇒ a real storm
        let a = step(&hot, &calm, 0.0);
        let b = step(&hot, &sheared, 0.0);
        assert!(b.organization >= a.organization, "more shear ⇒ at least as organized");
    }

    #[test]
    fn carrying_snow_across_steps_is_consistent() {
        let cold = climate(-0.9, 0.05);
        let mut depth = 0.0;
        for _ in 0..4 {
            depth = step(&cold, &inputs(), depth).snow_depth;
        }
        assert!(depth > 0.0, "the pack grows as cold snowy steps are chained");
    }

    #[test]
    fn deterministic() {
        let c = climate(0.2, 0.5);
        assert_eq!(step(&c, &inputs(), 1.0), step(&c, &inputs(), 1.0));
    }
}
