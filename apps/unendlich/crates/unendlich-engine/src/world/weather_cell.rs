//! `world::weather_cell` — compose the physically-grounded weather primitives
//! into one local weather cell: a rain-shadow-adjusted moisture drives cloud
//! cover, cloud cover plus moisture drive precipitation, moisture plus
//! instability drive storm intensity, and a pressure gradient drives wind.
//!
//! This is the physical counterpart to the time-driven [`crate::world::weather`]
//! model (which rolls weather off the clock). Here weather emerges from the
//! environment, so it can vary by terrain (a peak's lee is drier), altitude, and
//! local pressure. Ties together the previously-disjoint cores `rain_shadow`,
//! `cloud_coverage`, `precip_rate`, `storm_intensity`, and
//! `pressure_gradient_wind` — it composes them, it does not reimplement them.
//!
//! Pure and deterministic: data in, data out. No clock, no randomness, no I/O.

use super::{cloud_coverage, precip_rate, pressure_gradient_wind, rain_shadow, storm_intensity};

/// Environmental inputs for one weather cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellInputs {
    /// Windward (incoming) moisture `[0,1]` before any rain-shadow drying.
    pub windward_moisture: f32,
    /// Height of an upwind peak casting a rain shadow over this cell (0 = none).
    pub peak_height: f32,
    /// Rain-shadow dryness coefficient (drying per unit peak height).
    pub dryness: f32,
    /// Normalised temperature `[0,1]` (warmer holds fewer clouds).
    pub temp_norm: f32,
    /// Normalised altitude `[0,1]` (higher thins cloud).
    pub altitude_norm: f32,
    /// Atmospheric instability `[0,1]` (the storm driver).
    pub instability: f32,
    /// Pressure difference across the cell (drives geostrophic wind).
    pub pressure_diff: f32,
    /// Distance the pressure difference acts over.
    pub pressure_distance: f32,
    /// Wind coefficient.
    pub wind_coeff: f32,
}

/// The resolved local weather.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherCell {
    /// Effective moisture after the rain shadow `[0,1]`.
    pub moisture: f32,
    /// Cloud cover `[0,1]`.
    pub cloud: f32,
    /// Precipitation rate `[0,1]`.
    pub precip: f32,
    /// Storm intensity `[0,1]`.
    pub storm: f32,
    /// Wind speed `>= 0`.
    pub wind: f32,
}

/// Resolve a cell's weather from its environment, in physical order:
/// rain shadow → effective moisture → cloud cover → precipitation; plus storm
/// intensity (moisture × instability) and pressure-gradient wind. Pure and
/// deterministic; every field stays in range because each core clamps its own.
pub fn resolve_cell(i: &CellInputs) -> WeatherCell {
    let moisture = rain_shadow::leeward_moisture(i.windward_moisture, i.peak_height, i.dryness)
        .clamp(0.0, 1.0);
    let cloud = cloud_coverage::coverage(moisture, i.temp_norm, i.altitude_norm);
    let precip = precip_rate::rate(cloud, moisture);
    let storm = storm_intensity::intensity(moisture, i.instability);
    let wind = pressure_gradient_wind::wind_speed(i.pressure_diff, i.pressure_distance, i.wind_coeff);
    WeatherCell { moisture, cloud, precip, storm, wind }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> CellInputs {
        CellInputs {
            windward_moisture: 0.8,
            peak_height: 0.0,
            dryness: 0.0,
            temp_norm: 0.3,
            altitude_norm: 0.2,
            instability: 0.5,
            pressure_diff: 10.0,
            pressure_distance: 100.0,
            wind_coeff: 2.0,
        }
    }

    #[test]
    fn humid_cell_is_cloudy_and_wet() {
        let w = resolve_cell(&base());
        assert!(w.cloud > 0.3 && w.precip > 0.0, "humid cool cell clouds up and rains");
        assert!((0.0..=1.0).contains(&w.cloud) && (0.0..=1.0).contains(&w.precip));
    }

    #[test]
    fn rain_shadow_dries_the_lee() {
        let open = resolve_cell(&base());
        let mut shadowed = base();
        shadowed.peak_height = 3000.0;
        shadowed.dryness = 0.001;
        let lee = resolve_cell(&shadowed);
        assert!(lee.moisture < open.moisture, "a peak's lee has less moisture");
        assert!(lee.cloud <= open.cloud && lee.precip <= open.precip, "drier lee ⇒ fewer clouds, less rain");
    }

    #[test]
    fn storms_need_moisture_and_instability() {
        let mut calm = base();
        calm.instability = 0.0;
        assert_eq!(resolve_cell(&calm).storm, 0.0, "no instability ⇒ no storm");
        let mut stormy = base();
        stormy.instability = 0.95;
        assert!(resolve_cell(&stormy).storm > resolve_cell(&base()).storm, "more instability ⇒ stronger storm");
    }

    #[test]
    fn wind_tracks_the_pressure_gradient() {
        let calm = resolve_cell(&base());
        let mut windy = base();
        windy.pressure_diff = 40.0;
        assert!(resolve_cell(&windy).wind > calm.wind, "a steeper pressure gradient blows harder");
        assert!(calm.wind >= 0.0);
    }

    #[test]
    fn warmer_and_higher_thins_cloud() {
        let mut warm_high = base();
        warm_high.temp_norm = 0.9;
        warm_high.altitude_norm = 0.9;
        assert!(resolve_cell(&warm_high).cloud < resolve_cell(&base()).cloud);
    }

    #[test]
    fn deterministic_and_bounded() {
        let i = base();
        assert_eq!(resolve_cell(&i), resolve_cell(&i));
        for &m in &[0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let mut t = base();
            t.windward_moisture = m;
            let w = resolve_cell(&t);
            for v in [w.moisture, w.cloud, w.precip, w.storm] {
                assert!((0.0..=1.0).contains(&v) && v.is_finite());
            }
            assert!(w.wind.is_finite() && w.wind >= 0.0);
        }
    }
}
