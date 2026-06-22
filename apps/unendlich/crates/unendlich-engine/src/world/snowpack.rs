//! `world::snowpack` — the weather sim's temporal layer: a cell's precipitation
//! builds and melts a snow pack over time.
//!
//! [`crate::world::weather_cell`] gives an instantaneous precipitation rate; the
//! orphaned [`crate::world::snow_accumulate`] core integrates it into a snow
//! depth — below freezing falling precip adds to the pack, above freezing the
//! pack melts, never going negative. The one wrinkle is units: the weather cell
//! carries a normalized `[0,1]` temperature while `snow_accumulate` works in a
//! physical scale with freezing at 0, so [`to_celsius`] de-normalizes over the
//! caller's actual temperature range (no hidden magic threshold). Pure and
//! deterministic; the depth is the only state, passed in and returned.

use super::snow_accumulate;
use super::weather_cell::WeatherCell;

/// De-normalize a `[0,1]` climate temperature to °C over the caller's range
/// (`temp_min_c` at 0, `temp_max_c` at 1). With e.g. `-10..30`, freezing (0 °C)
/// falls at `temp_norm = 0.25`, so the coldest quarter accumulates snow.
pub fn to_celsius(temp_norm: f32, temp_min_c: f32, temp_max_c: f32) -> f32 {
    let t = temp_norm.clamp(0.0, 1.0);
    temp_min_c + (temp_max_c - temp_min_c) * t
}

/// Advance a cell's snow depth one step: its precipitation accumulates as snow
/// below freezing and the pack melts above, via `snow_accumulate::depth_step`.
/// `temp_c` is the ambient temperature in °C (see [`to_celsius`]). Pure.
pub fn step(depth: f32, cell: &WeatherCell, temp_c: f32, dt: f32) -> f32 {
    snow_accumulate::depth_step(depth, temp_c, cell.precip, dt)
}

/// As [`step`], but takes the cell's normalized temperature plus the caller's
/// °C range and de-normalizes it for you.
pub fn step_norm(
    depth: f32,
    cell: &WeatherCell,
    temp_norm: f32,
    temp_min_c: f32,
    temp_max_c: f32,
    dt: f32,
) -> f32 {
    step(depth, cell, to_celsius(temp_norm, temp_min_c, temp_max_c), dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(precip: f32) -> WeatherCell {
        WeatherCell { moisture: 0.8, cloud: 0.7, precip, storm: 0.0, wind: 0.3 }
    }

    #[test]
    fn to_celsius_maps_the_range() {
        assert!((to_celsius(0.0, -10.0, 30.0) + 10.0).abs() < 1e-6);
        assert!((to_celsius(1.0, -10.0, 30.0) - 30.0).abs() < 1e-6);
        let mid = to_celsius(0.5, -10.0, 30.0);
        assert!(mid > -10.0 && mid < 30.0);
    }

    #[test]
    fn cold_snow_builds_the_pack() {
        let c = cell(0.4);
        let mut depth = 0.0;
        for _ in 0..5 {
            depth = step(depth, &c, -5.0, 1.0); // below freezing
        }
        assert!(depth > 0.0, "snow accumulates while cold and precipitating");
    }

    #[test]
    fn warm_spell_melts_the_pack_to_zero() {
        let c = cell(0.0); // no new precip
        let mut depth = 3.0;
        for _ in 0..20 {
            depth = step(depth, &c, 10.0, 1.0); // well above freezing
        }
        assert_eq!(depth, 0.0, "a warm spell melts the pack and it never goes negative");
    }

    #[test]
    fn cold_and_dry_holds_steady() {
        let c = cell(0.0);
        let before = 2.0;
        let after = step(before, &c, -8.0, 1.0);
        assert!((after - before).abs() < 1e-6, "cold but dry ⇒ no change");
    }

    #[test]
    fn a_cold_then_warm_season_builds_then_melts() {
        let snowing = cell(0.5);
        let clearing = cell(0.0);
        let mut depth = 0.0;
        for _ in 0..6 {
            depth = step_norm(depth, &snowing, 0.1, -10.0, 30.0, 1.0); // norm 0.1 ⇒ -6 °C
        }
        let peak = depth;
        assert!(peak > 0.0, "the cold spell built a pack");
        for _ in 0..30 {
            depth = step_norm(depth, &clearing, 0.9, -10.0, 30.0, 1.0); // norm 0.9 ⇒ +26 °C
        }
        assert!(depth < peak && depth >= 0.0, "the warm spell melted it back down");
    }

    #[test]
    fn deterministic() {
        let c = cell(0.3);
        assert_eq!(step(1.0, &c, -2.0, 0.5), step(1.0, &c, -2.0, 0.5));
    }
}
