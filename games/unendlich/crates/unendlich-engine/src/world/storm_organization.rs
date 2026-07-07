//! `world::storm_organization` — vertical wind shear decides how *organized* a
//! cell's storm is.
//!
//! [`crate::world::weather_cell`] gives a convective storm intensity (moisture ×
//! instability). Real storms need a second ingredient to become severe: vertical
//! wind shear — wind that strengthens with height tilts and sustains the updraft,
//! turning a brief pulse storm into an organized, long-lived (supercellular) one.
//! This composes the orphaned [`crate::world::wind_shear`] core with that storm
//! intensity. Pure and deterministic.

use super::weather_cell::WeatherCell;
use super::wind_shear;

/// Fractional strengthening of the wind from the surface up to `top_altitude`
/// under shear coefficient `shear`: `(wind_aloft / surface) − 1`, read straight
/// off the `wind_shear` core with a unit base wind. Dimensionless, so it doesn't
/// depend on the surface wind's units; `0` at the surface or with no shear.
pub fn relative_shear(top_altitude: f32, shear: f32) -> f32 {
    (wind_shear::wind_at(1.0, top_altitude, shear) - 1.0).max(0.0)
}

/// How organized a storm is, in `[0,1]`. A storm needs both convective intensity
/// (`storm_intensity`, e.g. from `weather_cell`) and vertical wind shear to
/// organize. `0` when there's no storm; rises monotonically with each and
/// saturates in shear (`k` = half-saturation relative shear).
pub fn organization(storm_intensity: f32, relative_shear: f32) -> f32 {
    let s = storm_intensity.clamp(0.0, 1.0);
    let rs = relative_shear.max(0.0);
    let k = 0.5;
    let shear01 = rs / (rs + k);
    (s * shear01).clamp(0.0, 1.0)
}

/// Coarse storm mode for gameplay / VFX.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StormMode {
    /// No meaningful storm.
    Calm,
    /// A storm with little shear — short-lived, disorganized.
    Pulse,
    /// Sheared enough to organize and persist.
    Organized,
    /// Strongly sheared and intense — severe / supercellular.
    Severe,
}

/// Classify the storm. The exact cutoffs are tunable; the meaningful part is the
/// ordering — more intensity and shear ⇒ a more organized mode.
pub fn classify(storm_intensity: f32, organization: f32) -> StormMode {
    if storm_intensity < 0.1 {
        StormMode::Calm
    } else if organization < 0.15 {
        StormMode::Pulse
    } else if organization < 0.4 {
        StormMode::Organized
    } else {
        StormMode::Severe
    }
}

/// Organize a resolved cell's storm given a shear coefficient and the depth over
/// which it acts. Returns the `[0,1]` organization and the coarse mode.
pub fn organize_cell(cell: &WeatherCell, top_altitude: f32, shear: f32) -> (f32, StormMode) {
    let rs = relative_shear(top_altitude, shear);
    let org = organization(cell.storm, rs);
    (org, classify(cell.storm, org))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_shear_grows_with_height_and_shear() {
        assert_eq!(relative_shear(0.0, 1.0), 0.0, "no shear at the surface");
        assert!(relative_shear(3000.0, 0.8) > relative_shear(300.0, 0.8), "stronger aloft");
        assert!(relative_shear(2000.0, 1.0) > relative_shear(2000.0, 0.2), "more shear ⇒ more");
    }

    #[test]
    fn no_storm_never_organizes() {
        assert_eq!(organization(0.0, 5.0), 0.0, "no convective storm ⇒ no organization");
    }

    #[test]
    fn organization_rises_with_shear_and_intensity() {
        let low_shear = organization(0.8, relative_shear(800.0, 0.2));
        let high_shear = organization(0.8, relative_shear(4000.0, 2.0));
        assert!(high_shear > low_shear, "more shear organizes a given storm");
        let weak = organization(0.3, relative_shear(3000.0, 1.5));
        let strong = organization(0.9, relative_shear(3000.0, 1.5));
        assert!(strong > weak, "a more intense storm organizes more under the same shear");
        for v in [low_shear, high_shear, weak, strong] {
            assert!((0.0..=1.0).contains(&v) && v.is_finite());
        }
    }

    #[test]
    fn modes_order_from_calm_to_severe() {
        // no storm
        assert_eq!(classify(0.05, 0.9), StormMode::Calm);
        // a real storm with negligible shear stays a pulse
        let pulse = organize_cell(&cell(0.9), 500.0, 0.05);
        assert_eq!(pulse.1, StormMode::Pulse);
        // the same storm under strong deep-layer shear goes severe
        let severe = organize_cell(&cell(0.9), 4000.0, 2.0);
        assert_eq!(severe.1, StormMode::Severe);
        assert!(severe.0 > pulse.0, "severe carries the higher organization score");
    }

    #[test]
    fn deterministic() {
        let c = cell(0.7);
        assert_eq!(organize_cell(&c, 3000.0, 1.0), organize_cell(&c, 3000.0, 1.0));
    }

    // a minimal WeatherCell carrying just the storm field the API reads.
    fn cell(storm: f32) -> WeatherCell {
        WeatherCell { moisture: 0.7, cloud: 0.5, precip: 0.3, storm, wind: 0.4 }
    }
}
