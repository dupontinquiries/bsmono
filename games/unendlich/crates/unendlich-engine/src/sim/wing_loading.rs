//! sim::wing_loading — scale effects on flight (wing loading, stall, regime).
//!
//! From `design_library/04/flying_animal_behaviour` §1.2: mass scales as
//! length³ but wing area as length², so **wing loading** (mass ÷ wing area)
//! rises with size — which sets stall speed, manoeuvrability, and the whole
//! flight regime. `lift_force` and `glide_ratio` already give instantaneous lift
//! and L/D; this adds the size-dependent envelope they sit inside: how fast a
//! creature must move to stay airborne, and which aerodynamic regime it lives in
//! (a midge's viscous clap-and-fling vs a dragon's inertial, updraft-hungry
//! flight). Pure textbook aerodynamics, deterministic, std-only.

/// Sea-level air density (kg/m³).
pub const AIR_DENSITY: f32 = 1.225;
/// Gravity (m/s²).
pub const G: f32 = 9.81;

/// The aerodynamic regime a flyer operates in, ordered by increasing size /
/// wing loading (matches the doc's midge→dragon progression).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FlightRegime {
    /// Viscous, Re < 100 — clap-and-fling (midge).
    Viscous,
    /// Unsteady vortex attachment, Re ~1000 (honeybee).
    UnsteadyVortex,
    /// Mixed unsteady/steady (dragonfly).
    MixedUnsteady,
    /// Classical steady aerodynamics — bounding/soaring useful (sparrow→eagle).
    ClassicalSteady,
    /// Classical + inertial — high wing loading, needs updrafts (dragon).
    InertialAssisted,
}

/// Wing loading (kg/m²) = mass ÷ wing planform area. Guards zero area.
pub fn wing_loading(mass_kg: f32, wing_area_m2: f32) -> f32 {
    let a = wing_area_m2.max(1e-6);
    mass_kg.max(0.0) / a
}

/// Aspect ratio = wingspan² ÷ wing area. High AR → efficient glide.
pub fn aspect_ratio(wingspan_m: f32, wing_area_m2: f32) -> f32 {
    let a = wing_area_m2.max(1e-6);
    let s = wingspan_m.max(0.0);
    (s * s) / a
}

/// Stall speed (m/s): the minimum airspeed at which the wing can still generate
/// lift equal to weight, `v = √(2·m·g / (ρ·A·CL_max))`. Equivalent to
/// `√(2·wing_loading·g / (ρ·CL_max))`, so it rises with the square root of wing
/// loading — the core scale effect. `cl_max` is the max lift coefficient
/// (~1.2–1.8 for real wings).
pub fn stall_speed(mass_kg: f32, wing_area_m2: f32, cl_max: f32, air_density: f32) -> f32 {
    let a = wing_area_m2.max(1e-6);
    let rho = air_density.max(1e-3);
    let cl = cl_max.max(1e-3);
    let w = mass_kg.max(0.0) * G;
    (2.0 * w / (rho * a * cl)).sqrt()
}

/// Convenience: stall speed at sea level.
pub fn stall_speed_sea_level(mass_kg: f32, wing_area_m2: f32, cl_max: f32) -> f32 {
    stall_speed(mass_kg, wing_area_m2, cl_max, AIR_DENSITY)
}

/// Ratio between minimum-drag (max-range) speed and minimum-power speed: an
/// animal covering distance flies ~1.32× faster than one conserving energy.
pub const MIN_DRAG_OVER_MIN_POWER: f32 = 1.32;

/// Max-range (min-drag) speed from the min-power (energy-conserving) speed.
pub fn max_range_speed(min_power_speed: f32) -> f32 {
    min_power_speed.max(0.0) * MIN_DRAG_OVER_MIN_POWER
}

/// Min-power (energy-conserving) speed from the max-range (min-drag) speed.
pub fn min_power_speed(max_range_speed: f32) -> f32 {
    max_range_speed.max(0.0) / MIN_DRAG_OVER_MIN_POWER
}

/// Classify the flight regime from wing loading, using the doc's progression.
pub fn flight_regime(wing_loading_kg_m2: f32) -> FlightRegime {
    let wl = wing_loading_kg_m2;
    if wl < 0.01 {
        FlightRegime::Viscous
    } else if wl < 0.05 {
        FlightRegime::UnsteadyVortex
    } else if wl < 1.0 {
        FlightRegime::MixedUnsteady
    } else if wl < 70.0 {
        FlightRegime::ClassicalSteady
    } else {
        FlightRegime::InertialAssisted
    }
}

/// Does this flyer rely on updrafts / thermals to stay aloft economically?
/// True for the high-wing-loading inertial regime (a juvenile dragon "needs
/// updraft"), where sustained flapping flight is too costly.
pub fn needs_updraft(wing_loading_kg_m2: f32) -> bool {
    flight_regime(wing_loading_kg_m2) == FlightRegime::InertialAssisted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wing_loading_is_mass_over_area() {
        assert!((wing_loading(80.0, 4.0) - 20.0).abs() < 1e-4);
        // guards zero area (no NaN/inf)
        assert!(wing_loading(10.0, 0.0).is_finite());
    }

    #[test]
    fn aspect_ratio_is_span_squared_over_area() {
        // a 2 m span, 0.5 m² wing → AR 8.
        assert!((aspect_ratio(2.0, 0.5) - 8.0).abs() < 1e-4);
    }

    #[test]
    fn stall_speed_rises_with_the_square_root_of_wing_loading() {
        // Doubling wing loading (same wing, double mass) multiplies stall by √2.
        let light = stall_speed_sea_level(4.0, 1.0, 1.5);
        let heavy = stall_speed_sea_level(8.0, 1.0, 1.5);
        assert!((heavy / light - 2f32.sqrt()).abs() < 1e-3, "ratio {}", heavy / light);
    }

    #[test]
    fn stall_speed_matches_the_closed_form() {
        // v = √(2·m·g / (ρ·A·CL)); m=10, A=2, CL=1.5, ρ=1.225
        let v = stall_speed(10.0, 2.0, 1.5, 1.225);
        let expected = (2.0 * 10.0 * G / (1.225 * 2.0 * 1.5)).sqrt();
        assert!((v - expected).abs() < 1e-4);
    }

    #[test]
    fn thinner_air_raises_stall_speed() {
        let sea = stall_speed(10.0, 2.0, 1.5, 1.225);
        let altitude = stall_speed(10.0, 2.0, 1.5, 0.7); // thinner air
        assert!(altitude > sea, "must fly faster to stay up in thin air");
    }

    #[test]
    fn regimes_match_the_doc_progression() {
        assert_eq!(flight_regime(0.0005), FlightRegime::Viscous); // midge
        assert_eq!(flight_regime(0.02), FlightRegime::UnsteadyVortex); // bee
        assert_eq!(flight_regime(0.09), FlightRegime::MixedUnsteady); // dragonfly
        assert_eq!(flight_regime(20.0), FlightRegime::ClassicalSteady); // sparrow
        assert_eq!(flight_regime(55.0), FlightRegime::ClassicalSteady); // eagle
        assert_eq!(flight_regime(90.0), FlightRegime::InertialAssisted); // dragon
    }

    #[test]
    fn regime_is_monotonic_in_wing_loading() {
        let mut prev = FlightRegime::Viscous;
        for &wl in &[0.001_f32, 0.03, 0.5, 10.0, 100.0] {
            let r = flight_regime(wl);
            assert!(r >= prev, "regime must not regress as loading rises");
            prev = r;
        }
    }

    #[test]
    fn only_the_heaviest_flyers_need_updrafts() {
        assert!(!needs_updraft(20.0), "an eagle flaps fine");
        assert!(needs_updraft(90.0), "a heavy dragon needs thermals");
    }

    #[test]
    fn range_and_power_speeds_differ_by_the_documented_factor() {
        let vmp = 10.0;
        let vmd = max_range_speed(vmp);
        assert!((vmd - 13.2).abs() < 1e-3);
        assert!((min_power_speed(vmd) - vmp).abs() < 1e-3, "round-trips");
    }

    #[test]
    fn computations_are_deterministic() {
        assert_eq!(stall_speed_sea_level(80.0, 4.0, 1.4), stall_speed_sea_level(80.0, 4.0, 1.4));
        assert_eq!(flight_regime(90.0), flight_regime(90.0));
    }
}
