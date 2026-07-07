//! `world::orographic_precip` — the wind ↔ moisture ↔ weather bridge for
//! terrain-forced lift.
//!
//! [`crate::sim::wind_perturb::orographic_delta`] gives the mechanical **updraft**
//! where wind climbs a slope. Lifting moist air cools it toward saturation, so it
//! condenses into extra cloud and rain on the **windward** face (orographic
//! enhancement) and adds convective instability that feeds storms. The **lee**
//! dryness is already modelled by [`crate::world::rain_shadow`]; this is its missing
//! windward complement — together they make the full orographic dipole (wet upwind,
//! dry downwind).
//!
//! Pure and deterministic. It **composes** into
//! [`crate::world::weather_cell::CellInputs`] — augments moisture + instability so
//! `resolve_cell` yields the orographic-enhanced weather — it does not reimplement
//! cloud/precip/storm.

use crate::util::mathx::saturate;
use crate::world::weather_cell::CellInputs;

/// Condensation per unit updraft — how readily lifted air rains.
pub const LIFT_GAIN: f32 = 0.7;
/// How much mechanical uplift contributes to convective instability.
pub const UPLIFT_INSTABILITY_GAIN: f32 = 0.5;
/// Cap on the extra windward moisture uplift can inject.
pub const MAX_MOISTURE_BOOST: f32 = 0.35;

/// Windward condensation `[0,1]` from lifting moist air: rises with the updraft and
/// available moisture, and with **coolness** (cold air saturates sooner). Zero with
/// no lift, dry air, or air too warm to condense. Only *upward* motion condenses —
/// a downdraft (the lee) contributes nothing here (rain_shadow handles that).
pub fn condensation(updraft: f32, moisture01: f32, temp_norm: f32) -> f32 {
    let lift = updraft.max(0.0);
    let m = saturate(moisture01);
    let coolness = 1.0 - saturate(temp_norm); // colder ⇒ condenses more
    saturate(lift * LIFT_GAIN) * m * (0.4 + 0.6 * coolness)
}

/// Convective instability `[0,1]` added by the mechanical uplift of moist air —
/// forced ascent destabilises the column, priming storms.
pub fn uplift_instability(updraft: f32, moisture01: f32) -> f32 {
    saturate(updraft.max(0.0) * UPLIFT_INSTABILITY_GAIN) * saturate(moisture01)
}

/// Fold an orographic `updraft` into a weather cell's inputs: a wetter windward
/// column (capped) plus extra instability, so [`crate::world::weather_cell::resolve_cell`]
/// produces the orographic-enhanced cloud / precip / storm. A downdraft leaves the
/// inputs unchanged (the lee is dried by the rain-shadow term already in `base`).
pub fn augment_cell(base: CellInputs, updraft: f32) -> CellInputs {
    let cond = condensation(updraft, base.windward_moisture, base.temp_norm);
    let mut out = base;
    out.windward_moisture = saturate(base.windward_moisture + cond * MAX_MOISTURE_BOOST);
    out.instability = saturate(base.instability + uplift_instability(updraft, base.windward_moisture));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::weather_cell::resolve_cell;

    fn cell(moist: f32, temp: f32, instab: f32) -> CellInputs {
        CellInputs {
            windward_moisture: moist,
            peak_height: 0.0,
            dryness: 0.0,
            temp_norm: temp,
            altitude_norm: 0.2,
            instability: instab,
            pressure_diff: 10.0,
            pressure_distance: 100.0,
            wind_coeff: 2.0,
        }
    }

    #[test]
    fn no_lift_no_dry_air_no_condensation() {
        assert_eq!(condensation(0.0, 0.8, 0.3), 0.0); // no updraft
        assert_eq!(condensation(2.0, 0.0, 0.3), 0.0); // no moisture
        assert!(condensation(2.0, 0.8, 0.3) > 0.0); // lift + moisture ⇒ rain
    }

    #[test]
    fn colder_air_condenses_more() {
        let cold = condensation(2.0, 0.8, 0.0);
        let warm = condensation(2.0, 0.8, 1.0);
        assert!(cold > warm, "cold {cold} should exceed warm {warm}");
    }

    #[test]
    fn downdraft_does_not_condense() {
        assert_eq!(condensation(-3.0, 0.9, 0.2), 0.0);
    }

    #[test]
    fn windward_updraft_rains_more_than_flat() {
        let flat = resolve_cell(&cell(0.6, 0.3, 0.3));
        let windward = resolve_cell(&augment_cell(cell(0.6, 0.3, 0.3), 2.0));
        assert!(windward.precip > flat.precip, "orographic lift should boost windward rain");
        assert!(windward.storm >= flat.storm, "uplift should not reduce storminess");
    }

    #[test]
    fn orographic_dipole_wet_upwind_dry_downwind() {
        // upwind: lift, no rain shadow. downwind: a downdraft (no boost) + rain shadow.
        let upwind = resolve_cell(&augment_cell(cell(0.7, 0.25, 0.3), 2.5));
        let mut lee_in = cell(0.7, 0.25, 0.3);
        lee_in.peak_height = 2500.0;
        lee_in.dryness = 0.001;
        let lee = resolve_cell(&augment_cell(lee_in, -2.5)); // downdraft ⇒ no boost
        assert!(upwind.precip > lee.precip, "wet upwind {} vs dry lee {}", upwind.precip, lee.precip);
    }

    #[test]
    fn augment_stays_in_range() {
        let w = augment_cell(cell(0.95, 0.0, 0.9), 10.0);
        assert!((0.0..=1.0).contains(&w.windward_moisture));
        assert!((0.0..=1.0).contains(&w.instability));
    }
}
