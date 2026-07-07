//! Seasons — the slow annual cycle layered on top of the daily weather. A
//! year is a fixed number of in-game days; the `year_phase` (0..1) drives a
//! seasonal temperature swing, day-length, and which kind of precipitation
//! falls. This is what makes winter cold and snowy and summer warm and long,
//! and it's the gate the snow model reads: snow falls when the season +
//! climate put the effective temperature below freezing with enough humidity.
//!
//! Climate values are the engine's normalised `[0,1]` units (the `T`/`H` in the
//! run logs). Seasonal offsets are deltas in those units. Pure, unit-tested.

use std::f32::consts::TAU;

/// Default length of a year, in in-game days.
pub const DAYS_PER_YEAR: u32 = 96;

/// Peak seasonal temperature swing (normalised units) from the annual mean:
/// summer is `+AMPLITUDE`, winter `-AMPLITUDE`.
pub const TEMP_AMPLITUDE: f32 = 0.22;

/// Below this effective (normalised) temperature, precipitation is snow.
pub const FREEZING: f32 = 0.35;
/// Humidity must reach this for anything to fall at all.
pub const PRECIP_HUMIDITY: f32 = 0.5;

/// Phase of summer's warmth peak within the year (mid-summer). Winter's cold
/// trough is half a year opposite.
const SUMMER_PEAK: f32 = 0.375;

/// The four seasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

/// What's falling from the sky.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precipitation {
    Clear,
    Rain,
    Snow,
}

/// Year phase in `[0,1)` for a given day. Day 0 is the start of spring.
pub fn year_phase(day: u32, days_per_year: u32) -> f32 {
    let dpy = days_per_year.max(1);
    (day % dpy) as f32 / dpy as f32
}

impl Season {
    /// The season for a year phase. Quarters: Spring `[0,¼)`, Summer `[¼,½)`,
    /// Autumn `[½,¾)`, Winter `[¾,1)`.
    pub fn from_phase(phase: f32) -> Season {
        match (phase.rem_euclid(1.0) * 4.0) as u32 {
            0 => Season::Spring,
            1 => Season::Summer,
            2 => Season::Autumn,
            _ => Season::Winter,
        }
    }
}

/// Seasonal temperature offset (normalised units) at `phase`. Peaks `+AMP` in
/// midsummer, troughs `-AMP` in midwinter; spring/autumn are near neutral.
/// `southern` flips the calendar for the southern hemisphere.
pub fn temperature_offset(phase: f32, southern: bool) -> f32 {
    let p = if southern { phase + 0.5 } else { phase };
    TEMP_AMPLITUDE * (TAU * (p - SUMMER_PEAK)).cos()
}

/// Effective temperature = annual-mean base climate temp + the seasonal swing,
/// clamped to `[0,1]`.
pub fn effective_temperature(base_temp: f32, phase: f32, southern: bool) -> f32 {
    (base_temp + temperature_offset(phase, southern)).clamp(0.0, 1.0)
}

/// Fraction of the day that is daylight at `phase` and `latitude_deg`
/// (0 = equator, ±90 = poles). The equator stays near `0.5`; higher latitudes
/// swing harder — long summer days, short winter days. Clamped away from the
/// 0/1 extremes so dawn/dusk never fully vanish.
pub fn daylight_fraction(phase: f32, latitude_deg: f32, southern: bool) -> f32 {
    let p = if southern { phase + 0.5 } else { phase };
    let swing = (latitude_deg.abs() / 90.0).clamp(0.0, 1.0) * 0.45;
    (0.5 + swing * (TAU * (p - SUMMER_PEAK)).cos()).clamp(0.05, 0.95)
}

/// What precipitation falls given an effective temperature and humidity (both
/// normalised). Dry air → Clear; otherwise Snow below freezing, else Rain.
pub fn precipitation_for(effective_temp: f32, humidity: f32) -> Precipitation {
    if humidity < PRECIP_HUMIDITY {
        Precipitation::Clear
    } else if effective_temp < FREEZING {
        Precipitation::Snow
    } else {
        Precipitation::Rain
    }
}

/// Convenience: resolve the full seasonal picture for a day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeasonState {
    pub phase: f32,
    pub season: Season,
    pub effective_temp: f32,
    pub daylight: f32,
    pub precipitation: Precipitation,
}

/// Combine day + base climate into the seasonal state the renderer and the
/// snow model consume.
pub fn resolve(
    day: u32,
    days_per_year: u32,
    base_temp: f32,
    humidity: f32,
    latitude_deg: f32,
    southern: bool,
) -> SeasonState {
    let phase = year_phase(day, days_per_year);
    let effective_temp = effective_temperature(base_temp, phase, southern);
    SeasonState {
        phase,
        season: Season::from_phase(phase),
        effective_temp,
        daylight: daylight_fraction(phase, latitude_deg, southern),
        precipitation: precipitation_for(effective_temp, humidity),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn phase_and_season_quarters() {
        assert!(approx(year_phase(0, 96), 0.0, 1e-6));
        assert!(approx(year_phase(48, 96), 0.5, 1e-6));
        assert!(approx(year_phase(96, 96), 0.0, 1e-6), "wraps at year end");
        assert_eq!(Season::from_phase(0.1), Season::Spring);
        assert_eq!(Season::from_phase(0.3), Season::Summer);
        assert_eq!(Season::from_phase(0.6), Season::Autumn);
        assert_eq!(Season::from_phase(0.9), Season::Winter);
    }

    #[test]
    fn summer_is_warmer_than_winter() {
        let summer = temperature_offset(SUMMER_PEAK, false);
        let winter = temperature_offset(SUMMER_PEAK + 0.5, false);
        assert!(approx(summer, TEMP_AMPLITUDE, 1e-4), "summer near +AMP");
        assert!(approx(winter, -TEMP_AMPLITUDE, 1e-4), "winter near −AMP");
        assert!(summer > winter);
        // Spring/autumn are near the annual mean.
        assert!(temperature_offset(0.125, false).abs() < 0.05, "spring ~neutral");
    }

    #[test]
    fn southern_hemisphere_is_inverted() {
        let north = temperature_offset(SUMMER_PEAK, false);
        let south = temperature_offset(SUMMER_PEAK, true);
        assert!(north > 0.0 && south < 0.0, "same date: north summer, south winter");
        assert!(approx(north, -south, 1e-4));
    }

    #[test]
    fn daylight_swings_with_latitude() {
        // Equator: nearly constant half-and-half all year.
        assert!(approx(daylight_fraction(SUMMER_PEAK, 0.0, false), 0.5, 0.02));
        assert!(approx(daylight_fraction(SUMMER_PEAK + 0.5, 0.0, false), 0.5, 0.02));
        // High latitude: long summer day, short winter day.
        let hi_summer = daylight_fraction(SUMMER_PEAK, 70.0, false);
        let hi_winter = daylight_fraction(SUMMER_PEAK + 0.5, 70.0, false);
        assert!(hi_summer > 0.6 && hi_winter < 0.4, "{hi_summer} vs {hi_winter}");
        // Never fully vanishes.
        assert!(daylight_fraction(SUMMER_PEAK + 0.5, 90.0, false) >= 0.05);
    }

    #[test]
    fn precipitation_depends_on_temp_and_humidity() {
        assert_eq!(precipitation_for(0.2, 0.3), Precipitation::Clear, "dry → clear");
        assert_eq!(precipitation_for(0.2, 0.8), Precipitation::Snow, "cold + humid → snow");
        assert_eq!(precipitation_for(0.7, 0.8), Precipitation::Rain, "warm + humid → rain");
    }

    #[test]
    fn resolve_winter_high_latitude_snows() {
        // Midwinter, far north, cool base climate, humid: cold enough to snow.
        let winter_day = (DAYS_PER_YEAR as f32 * (SUMMER_PEAK + 0.5)) as u32;
        let st = resolve(winter_day, DAYS_PER_YEAR, 0.45, 0.7, 65.0, false);
        assert_eq!(st.season, Season::Winter);
        assert!(st.effective_temp < FREEZING, "winter pulls temp below freezing");
        assert_eq!(st.precipitation, Precipitation::Snow);
        assert!(st.daylight < 0.45, "short winter days up north");
        // The same humid climate in high summer rains instead.
        let summer_day = (DAYS_PER_YEAR as f32 * SUMMER_PEAK) as u32;
        let su = resolve(summer_day, DAYS_PER_YEAR, 0.45, 0.7, 65.0, false);
        assert_eq!(su.precipitation, Precipitation::Rain);
    }
}
