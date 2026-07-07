//! `sim::salinity` (r459) — salinity as a first-class environmental field.
//!
//! The engine had temperature, humidity and erosion but no concept of how SALTY water is. Salinity
//! (0 = fresh, 1 = briny) is a derived field: the open sea is salty, hot arid flat basins concentrate
//! salt by evaporation (salt flats), and cold or very wet inland water is fresh. Pure + deterministic,
//! so it can drive water tint now and creature salinity-tolerance genes / spawn habitat later.

#[inline]
fn clamp01(x: f32) -> f32 { x.max(0.0).min(1.0) }

/// Salinity `0..1` from climate, height relative to sea level (m; negative = below), and the worldgen
/// erosion field (`0..1`; low erosion = flat basins that trap evaporating water).
pub fn salinity01(temp01: f32, humidity01: f32, elevation_rel: f32, erosion01: f32) -> f32 {
    let t = clamp01(temp01);
    let h = clamp01(humidity01);
    let e = clamp01(erosion01);
    // Marine: 1 at/below sea level, fading to 0 by +10 m.
    let sea = clamp01(1.0 - elevation_rel.max(0.0) / 10.0);
    // Evaporite (inland salt flats): hot, dry, flat.
    let evap = t.powf(1.5) * (1.0 - h) * (1.0 - e);
    let raw = sea.max(evap);
    // Cold / very wet inland freshens; does not freshen the open sea.
    let freshen = (1.0 - sea) * (h * 0.4 + (1.0 - t) * 0.25);
    clamp01(raw - freshen)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn deep_ocean_is_briny() {
        assert!(salinity01(0.6, 0.7, -5.0, 0.3) > 0.85);
        assert!(salinity01(0.5, 0.5, 0.0, 0.3) > 0.85);
    }
    #[test] fn high_wet_lake_is_fresh() { assert!(salinity01(0.3, 0.9, 25.0, 0.4) < 0.2); }
    #[test] fn hot_arid_flat_basin_is_salty() { assert!(salinity01(0.95, 0.05, 5.0, 0.1) > 0.5); }
    #[test] fn cold_freshens_inland() {
        assert!(salinity01(0.1, 0.6, 8.0, 0.3) < salinity01(0.9, 0.6, 8.0, 0.3));
    }
    #[test] fn deeper_not_less_salty() {
        assert!(salinity01(0.5, 0.5, -2.0, 0.3) >= salinity01(0.5, 0.5, 6.0, 0.3));
    }
}
