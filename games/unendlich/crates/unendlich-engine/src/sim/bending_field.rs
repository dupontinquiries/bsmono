//! `sim::bending_field` — sample the seven ambient bending properties (H, R, W,
//! P, L, S, B) from raw world state (§2.1). These are the field values
//! `affinity_access` gates disciplines on: this module is the *producer*, turning
//! biome/weather/geometry inputs into the normalized `Fields` that the access
//! gate consumes.
//!
//! | | property | driven by |
//! |-|----------|-----------|
//! | H | Humidity        | biome moisture, rain, open-water proximity |
//! | R | Radiant heat    | solar altitude, biome temperature, volcanic proximity |
//! | W | Wind velocity   | weather wind, altitude — damped to ~0 when enclosed (caves) |
//! | P | Pressure        | ≈0.5 at the surface, rises with depth, falls with altitude |
//! | L | Luminosity      | sky light, dimmed by weather and blocking geometry |
//! | S | Seismic index   | depth, volcanic proximity, recent impact activity |
//! | B | Biotic density  | nearby living entities and vegetation mass |
//!
//! Each output is clamped to `0..1`. The weights below are a reasonable model of
//! the spec's described behaviour (surface pressure ≈0.5, caves have no wind,
//! volcanic depths read hot and seismic, submersion reads fully humid) and are
//! meant to be tuned; the *sources* each property draws on are the fixed part.
//!
//! Pure/std-only. Returns `affinity_access::Fields` so it plugs straight into the
//! access gate.

use crate::sim::affinity_access::Fields;

/// Raw world inputs, each already normalized to `0..1` (except `weather_front`).
#[derive(Debug, Clone, Copy, Default)]
pub struct RawEnv {
    // Humidity sources
    pub biome_moisture: f32,
    pub rain_intensity: f32,
    pub water_proximity: f32,
    // Radiant-heat sources
    pub solar_altitude: f32,
    pub biome_temperature: f32,
    pub volcanic_proximity: f32,
    // Wind sources
    pub weather_wind: f32,
    pub altitude01: f32,
    /// 1.0 = fully enclosed (cave), damping wind toward 0.
    pub enclosure: f32,
    // Pressure sources
    pub depth01: f32,
    /// Weather-front bias, `-1..1` (low → high pressure system).
    pub weather_front: f32,
    // Luminosity sources
    pub sky_light: f32,
    /// Cloud/storm dimming, `0..1`.
    pub weather_dim: f32,
    /// Overhead blocking geometry, `0..1`.
    pub geometry_block: f32,
    // Seismic sources
    pub impact_activity: f32,
    // Biotic sources
    pub entity_density: f32,
    pub vegetation_mass: f32,
}

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// Humidity: biome moisture + rain + open-water proximity. Submersion reads high.
pub fn humidity(e: &RawEnv) -> f32 {
    clamp01(0.4 * e.biome_moisture + 0.35 * e.rain_intensity + 0.6 * e.water_proximity)
}

/// Radiant heat: sun angle + biome temperature + volcanic proximity (which
/// dominates — a volcano is hot at midnight).
pub fn radiant_heat(e: &RawEnv) -> f32 {
    clamp01(0.4 * e.solar_altitude + 0.3 * e.biome_temperature + 0.7 * e.volcanic_proximity)
}

/// Wind velocity: weather wind + altitude, multiplied down by enclosure so a
/// sealed cave reads ~0 regardless of the surface weather.
pub fn wind(e: &RawEnv) -> f32 {
    clamp01((0.6 * e.weather_wind + 0.4 * e.altitude01) * (1.0 - clamp01(e.enclosure)))
}

/// Barometric pressure: ≈0.5 at the surface, up with depth, down with altitude,
/// nudged by the weather front.
pub fn pressure(e: &RawEnv) -> f32 {
    clamp01(0.5 + 0.5 * e.depth01 - 0.5 * e.altitude01 + 0.1 * e.weather_front)
}

/// Luminosity: sky light attenuated by weather dimming and blocking geometry.
pub fn luminosity(e: &RawEnv) -> f32 {
    clamp01(e.sky_light * (1.0 - 0.5 * clamp01(e.weather_dim)) * (1.0 - clamp01(e.geometry_block)))
}

/// Seismic index: depth + volcanic proximity + recent impact activity.
pub fn seismic(e: &RawEnv) -> f32 {
    clamp01(0.6 * e.depth01 + 0.6 * e.volcanic_proximity + 0.3 * e.impact_activity)
}

/// Biotic density: nearby entities + vegetation mass.
pub fn biotic(e: &RawEnv) -> f32 {
    clamp01(0.6 * e.entity_density + 0.6 * e.vegetation_mass)
}

/// Sample all seven properties into the `Fields` the access gate consumes.
pub fn sample(e: &RawEnv) -> Fields {
    Fields {
        h: humidity(e),
        r: radiant_heat(e),
        w: wind(e),
        p: pressure(e),
        l: luminosity(e),
        s: seismic(e),
        b: biotic(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::affinity_access;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn surface_pressure_is_about_half() {
        let e = RawEnv::default(); // depth 0, altitude 0, no front
        assert!(approx(pressure(&e), 0.5), "flat surface ≈ 0.5");
        // deep is higher, high altitude lower
        let deep = RawEnv { depth01: 1.0, ..Default::default() };
        let sky = RawEnv { altitude01: 1.0, ..Default::default() };
        assert!(pressure(&deep) > 0.5 && pressure(&sky) < 0.5);
    }

    #[test]
    fn caves_have_no_wind_even_in_a_gale() {
        let gale_cave = RawEnv { weather_wind: 1.0, altitude01: 1.0, enclosure: 1.0, ..Default::default() };
        assert!(approx(wind(&gale_cave), 0.0), "sealed cave damps wind to 0");
        let open_gale = RawEnv { weather_wind: 1.0, enclosure: 0.0, ..Default::default() };
        assert!(wind(&open_gale) > 0.5);
    }

    #[test]
    fn luminosity_tracks_sky_and_dimming() {
        let noon = RawEnv { sky_light: 1.0, ..Default::default() };
        assert!(approx(luminosity(&noon), 1.0), "clear noon = full light");
        let night = RawEnv { sky_light: 0.0, ..Default::default() };
        assert!(approx(luminosity(&night), 0.0));
        let covered = RawEnv { sky_light: 1.0, geometry_block: 1.0, ..Default::default() };
        assert!(approx(luminosity(&covered), 0.0), "roofed over = dark");
    }

    #[test]
    fn volcanic_depth_reads_hot_and_seismic() {
        let magma = RawEnv { depth01: 0.9, volcanic_proximity: 1.0, ..Default::default() };
        assert!(radiant_heat(&magma) > 0.6, "volcanic = hot even at depth/night");
        assert!(seismic(&magma) > 0.7, "deep + volcanic = high seismic");
    }

    #[test]
    fn submersion_reads_fully_humid() {
        let underwater = RawEnv { water_proximity: 1.0, biome_moisture: 1.0, ..Default::default() };
        assert!(humidity(&underwater) > 0.9);
        let desert = RawEnv { biome_moisture: 0.05, ..Default::default() };
        assert!(humidity(&desert) < 0.1);
    }

    #[test]
    fn all_outputs_clamp_to_unit_range() {
        // pile every source to the max — nothing exceeds 1 or drops below 0
        let maxed = RawEnv {
            biome_moisture: 1.0, rain_intensity: 1.0, water_proximity: 1.0,
            solar_altitude: 1.0, biome_temperature: 1.0, volcanic_proximity: 1.0,
            weather_wind: 1.0, altitude01: 1.0, enclosure: 0.0,
            depth01: 1.0, weather_front: 1.0,
            sky_light: 1.0, weather_dim: 0.0, geometry_block: 0.0,
            impact_activity: 1.0, entity_density: 1.0, vegetation_mass: 1.0,
        };
        let f = sample(&maxed);
        for v in [f.h, f.r, f.w, f.p, f.l, f.s, f.b] {
            assert!((0.0..=1.0).contains(&v), "field {v} out of range");
        }
    }

    #[test]
    fn feeds_the_access_gate_end_to_end() {
        // A deep volcanic vent: hot + seismic + high pressure → Earth (idx 3,
        // needs p>0.6) and the seismic-heavy disciplines should be reachable,
        // while an airy discipline (wind idx 2) should NOT unlock underground.
        let vent = RawEnv {
            depth01: 0.95,
            volcanic_proximity: 1.0,
            biome_temperature: 0.8,
            enclosure: 1.0, // sealed → no wind
            ..Default::default()
        };
        let f = sample(&vent);
        // wind field below its 0.40 threshold → air-type locked
        assert!(!affinity_access::can_access(&f, 2), "no wind underground → locked");
        // seismic field above its 0.50 threshold → seismic disciplines unlocked
        assert!(affinity_access::can_access(&f, 5), "deep volcanic → seismic unlocked");
        // a breezy hilltop flips wind on
        let hill = RawEnv { weather_wind: 0.9, altitude01: 0.8, enclosure: 0.0, ..Default::default() };
        assert!(affinity_access::can_access(&sample(&hill), 2), "windy hill → air unlocked");
    }
}
