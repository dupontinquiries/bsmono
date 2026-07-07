//! `world::weather_front` — a weather front moving across a line of cells.
//!
//! Two pieces of physics that the single-cell [`crate::world::weather_cell`]
//! can't express on its own:
//!
//! * **A temperature front.** Warm air on the upwind (low-x) side, cold air on
//!   the downwind side, smoothstepped across a transition zone — the orphaned
//!   [`crate::world::front_temperature_gradient`] core. The front rides the wind
//!   ([`Front::advanced`]), so a cell warms or cools as the front passes.
//! * **Moisture advection.** Wind carries moisture downwind; [`advect_moisture`]
//!   is a donor-cell (upwind) step that is mass-conserving up to what enters and
//!   leaves the domain and never manufactures new extrema.
//!
//! [`resolve_strip`] then composes both with `weather_cell` to give the weather
//! along the whole strip. Pure and deterministic throughout — it ties the cores
//! together, it does not reimplement them.

use super::front_temperature_gradient;
use super::weather_cell::{self, CellInputs, WeatherCell};

/// A temperature front: warm air mass on the low-x side, cold on the high-x
/// side, transitioning across `width` centred on `center`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Front {
    /// World-x of the front centre.
    pub center: f32,
    /// Temperature of the warm air mass (low-x side).
    pub warm_temp: f32,
    /// Temperature of the cold air mass (high-x side).
    pub cold_temp: f32,
    /// Width of the transition zone (`> 0`).
    pub width: f32,
}

impl Front {
    /// The front after riding the wind for `dt` at `wind_speed` (downwind, +x).
    pub fn advanced(&self, wind_speed: f32, dt: f32) -> Front {
        Front { center: self.center + wind_speed * dt, ..*self }
    }

    /// Temperature at world-x `x`: warm on the low-x side, cold on the high-x
    /// side, smoothstepped across the front via `front_temperature_gradient`.
    pub fn temperature_at(&self, x: f32) -> f32 {
        let w = self.width.max(1e-4);
        // 0 at the warm edge (center - w/2), w at the cold edge (center + w/2).
        let position = x - (self.center - 0.5 * w);
        front_temperature_gradient::temperature(self.warm_temp, self.cold_temp, position, w)
    }
}

/// Donor-cell (upwind) advection of a moisture field for wind blowing toward
/// +x. `inflow` is the moisture crossing the upwind (low-x) boundary; moisture
/// at the downwind end leaves the domain. `courant` is the per-step CFL fraction
/// (clamped to `[0,1]`) — the fraction of a cell the field shifts. Each output is
/// a convex blend of a cell and its upwind neighbour, so the scheme is monotone
/// (no new maxima/minima) and a uniform field with `inflow` equal to it is
/// stationary.
pub fn advect_moisture(moisture: &[f32], inflow: f32, courant: f32) -> Vec<f32> {
    let c = courant.clamp(0.0, 1.0);
    let mut out = Vec::with_capacity(moisture.len());
    let mut upstream = inflow;
    for &m in moisture {
        out.push(m * (1.0 - c) + upstream * c);
        upstream = m;
    }
    out
}

/// The shared atmosphere applied to every cell of a strip (everything
/// `weather_cell` needs except the per-cell moisture and temperature, which come
/// from the advected field and the front).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StripAtmo {
    pub peak_height: f32,
    pub dryness: f32,
    pub altitude_norm: f32,
    pub instability: f32,
    pub pressure_diff: f32,
    pub pressure_distance: f32,
    pub wind_coeff: f32,
}

/// Resolve the weather along a strip. `xs` and `moisture` are per-cell and must
/// be the same length; `temp_lo`/`temp_hi` map the front's temperature into the
/// `[0,1]` band `weather_cell` expects. Each cell uses its advected moisture as
/// windward moisture and the front temperature at its x. Pure; cells beyond the
/// shorter of `xs`/`moisture` are ignored.
pub fn resolve_strip(
    xs: &[f32],
    moisture: &[f32],
    front: &Front,
    temp_lo: f32,
    temp_hi: f32,
    atmo: &StripAtmo,
) -> Vec<WeatherCell> {
    let span = (temp_hi - temp_lo).abs().max(1e-4);
    xs.iter()
        .zip(moisture.iter())
        .map(|(&x, &m)| {
            let temp_norm = ((front.temperature_at(x) - temp_lo) / span).clamp(0.0, 1.0);
            weather_cell::resolve_cell(&CellInputs {
                windward_moisture: m,
                peak_height: atmo.peak_height,
                dryness: atmo.dryness,
                temp_norm,
                altitude_norm: atmo.altitude_norm,
                instability: atmo.instability,
                pressure_diff: atmo.pressure_diff,
                pressure_distance: atmo.pressure_distance,
                wind_coeff: atmo.wind_coeff,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front() -> Front {
        Front { center: 50.0, warm_temp: 1.0, cold_temp: 0.0, width: 20.0 }
    }

    #[test]
    fn front_is_warm_ahead_cold_behind() {
        let f = front();
        assert!((f.temperature_at(0.0) - 1.0).abs() < 1e-6, "far upwind is fully warm");
        assert!((f.temperature_at(100.0) - 0.0).abs() < 1e-6, "far downwind is fully cold");
        let mid = f.temperature_at(50.0);
        assert!(mid > 0.0 && mid < 1.0, "the front centre is a blend");
    }

    #[test]
    fn advancing_front_warms_a_downwind_cell() {
        // Warm air is upwind; as the pattern rides the wind downwind, the warm
        // mass advances into cells ahead of it (a warm front), so they warm.
        let f = front();
        let x = 60.0; // starts on the cold side
        let before = f.temperature_at(x);
        let after = f.advanced(5.0, 2.0).temperature_at(x); // front moves +10 toward +x
        assert!(after > before, "as the warm front advances, a downwind cell warms");
    }

    #[test]
    fn uniform_moisture_is_stationary() {
        let m = vec![0.6f32; 5];
        let out = advect_moisture(&m, 0.6, 0.4);
        for v in out {
            assert!((v - 0.6).abs() < 1e-6, "uniform field with matching inflow doesn't change");
        }
    }

    #[test]
    fn a_moist_pulse_moves_downwind() {
        // dry strip with one wet cell; after a step the wetness bleeds to +x.
        let m = vec![0.0, 1.0, 0.0, 0.0];
        let out = advect_moisture(&m, 0.0, 0.5);
        assert!(out[1] < 1.0, "the source cell sheds moisture");
        assert!(out[2] > 0.0, "its downwind neighbour gains moisture");
        assert!(out[3] <= out[2], "the front edge of the pulse leads");
    }

    #[test]
    fn advection_is_monotone() {
        let m = vec![0.2, 0.9, 0.1, 0.5];
        let out = advect_moisture(&m, 0.2, 0.7);
        let (lo, hi) = (0.1f32, 0.9f32);
        for v in out {
            assert!((lo - 1e-6..=hi + 1e-6).contains(&v), "no value outside the input range");
        }
    }

    #[test]
    fn strip_resolves_bounded_weather_with_a_front() {
        let xs = vec![0.0, 25.0, 50.0, 75.0, 100.0];
        let moisture = vec![0.8, 0.8, 0.8, 0.8, 0.8];
        let atmo = StripAtmo {
            peak_height: 0.0,
            dryness: 0.0,
            altitude_norm: 0.2,
            instability: 0.5,
            pressure_diff: 10.0,
            pressure_distance: 100.0,
            wind_coeff: 2.0,
        };
        let cells = resolve_strip(&xs, &moisture, &front(), 0.0, 1.0, &atmo);
        assert_eq!(cells.len(), 5);
        for c in &cells {
            for v in [c.moisture, c.cloud, c.precip, c.storm] {
                assert!((0.0..=1.0).contains(&v) && v.is_finite());
            }
            assert!(c.wind.is_finite() && c.wind >= 0.0);
        }
        // the front's temperature only drives cloud here (instability is a
        // shared input): the cold downwind end is cloudier than the warm upwind end.
        assert!(cells[4].cloud > cells[0].cloud, "cold side of the front is cloudier");
    }

    #[test]
    fn deterministic() {
        let xs = vec![10.0, 40.0, 70.0];
        let m = vec![0.5, 0.7, 0.3];
        let atmo = StripAtmo {
            peak_height: 0.0, dryness: 0.0, altitude_norm: 0.1, instability: 0.4,
            pressure_diff: 8.0, pressure_distance: 80.0, wind_coeff: 1.5,
        };
        assert_eq!(
            resolve_strip(&xs, &m, &front(), 0.0, 1.0, &atmo),
            resolve_strip(&xs, &m, &front(), 0.0, 1.0, &atmo),
        );
    }
}
